//! PUT /v1/files: stream the request body straight into the content store
//! (one pass, hashing en route) and register the record with a bumped blob
//! reference, atomically. The body is never buffered: the cap is enforced
//! by a reading wrapper, so memory stays bounded by the ingest chunk size
//! no matter how large the upload claims to be.

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::acl::{Action, SpacePath};
use crate::auth::AuthPrincipal;
use crate::state::AppState;
use futures_util::FutureExt as _;
use http_body_util::BodyExt as _;
use kallip_agora_common::principal::Principal;
use kallip_common::protocol::ApiError;

#[derive(Debug, Deserialize)]
pub struct PutQuery {
    /// Destination space path (e.g. `/users/{user}/shared/report.pdf`).
    pub path: String,
}

/// The response body of a successful upload.
#[derive(Debug, Serialize)]
pub struct PutResponse {
    /// The server-minted record id (the addressability handle).
    pub record_id: uuid::Uuid,
    /// The content address (SHA-256 based); re-uploading identical bytes
    /// lands on the same id and deduplicates by construction.
    pub blob_id: String,
}

/// PUT /v1/files?path=...
pub async fn put_file(
    State(state): State<AppState>,
    Query(query): Query<PutQuery>,
    AuthPrincipal(principal): AuthPrincipal,
    body: Body,
) -> Result<Response, ApiError> {
    let path = SpacePath::parse(&query.path).ok_or_else(|| {
        ApiError::bad_request("path must be a space path like /users/{user}/shared/name")
    })?;
    let owner = match &principal {
        Principal::User(user) => {
            if !crate::acl::user_can(user.as_ref(), &path, Action::Write) {
                return Err(ApiError::forbidden("not allowed on this path"));
            }
            user.to_string()
        }
        Principal::Tagma(tagma) => {
            let facts = super::tagma_facts(&state, tagma).await?;
            if !crate::acl::tagma_can(tagma.as_ref(), &path, Action::Write, &facts) {
                return Err(ApiError::forbidden("not allowed on this path"));
            }
            tagma.to_string()
        }
        Principal::Admin => {
            return Err(ApiError::forbidden("admin cannot upload content"));
        }
    };

    // Quota: a deliberate placeholder, off by design -- the plan's single
    // registered stub. Removal point: the Q2 quota ruling (which settles
    // the granularity); until then no space is quota-checked and uploads
    // are bounded only by the body cap below.

    let blob_id = {
        let mut reader = CappedBody::new(body, state.config.max_body_bytes);
        match state.blob.put(&mut reader).await {
            Ok(blob_id) => blob_id,
            Err(e) => {
                if reader.exceeded() {
                    return Err(ApiError {
                        status: StatusCode::PAYLOAD_TOO_LARGE.as_u16(),
                        message: format!(
                            "body exceeds the configured maximum of {} bytes",
                            state.config.max_body_bytes
                        ),
                    });
                }
                return Err(ApiError::internal(e));
            }
        }
    };

    let size = state
        .blob
        .stat(&blob_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("just-stored blob is missing"))?
        .size;
    let size = size as i64;
    let record_id = crate::metadata::repo::register_upload(
        &state.db,
        &blob_id,
        size,
        &query.path,
        &owner,
        None,
    )
    .await
    .map_err(ApiError::internal)?;

    Ok((
        StatusCode::CREATED,
        axum::Json(PutResponse {
            record_id,
            blob_id: blob_id.to_string(),
        }),
    )
        .into_response())
}

use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use axum::body::Bytes;
use tokio::io::{AsyncRead, ReadBuf};

/// An [`AsyncRead`] over an axum [`Body`] that refuses to deliver more than
/// `cap` bytes: the first byte past the cap turns into an IO error and
/// `exceeded` is set, so the caller can map the failure to 413. Byte flow
/// stays frame-sized end to end -- nothing here buffers the body.
pub(crate) struct CappedBody {
    body: Body,
    /// Remainder of the last polled data frame.
    pending: Bytes,
    /// How many more bytes the cap allows.
    remaining: u64,
    /// Set once the cap has been crossed; every later read errors.
    exceeded: bool,
    /// Set when the body signalled end-of-stream.
    eof: bool,
}

impl CappedBody {
    pub fn new(body: Body, cap: u64) -> Self {
        Self {
            body,
            pending: Bytes::new(),
            remaining: cap,
            exceeded: false,
            eof: false,
        }
    }

    /// Whether the body crossed the cap (as opposed to failing for another
    /// reason). Read after `put` returns an error.
    pub fn exceeded(&self) -> bool {
        self.exceeded
    }

    fn overflow_error() -> io::Error {
        io::Error::other("body exceeds the configured maximum")
    }

    /// Copy up to `max` bytes of `data` into `buf`; keep the rest pending.
    /// Returns the number of bytes copied.
    fn serve(&mut self, data: &[u8], buf: &mut ReadBuf<'_>, max: usize) -> usize {
        let take = data.len().min(max).min(buf.remaining());
        buf.put_slice(&data[..take]);
        if take < data.len() {
            self.pending = Bytes::copy_from_slice(&data[take..]);
        } else {
            // Fully consumed: clear the remainder, or a boundary-exact
            // read would leave stale bytes that look like overflow.
            self.pending = Bytes::new();
        }
        self.remaining -= take as u64;
        take
    }
}

impl AsyncRead for CappedBody {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if this.exceeded {
            return Poll::Ready(Err(Self::overflow_error()));
        }
        // Serve from the previous frame's remainder first.
        if !this.pending.is_empty() {
            if this.remaining == 0 {
                this.exceeded = true;
                return Poll::Ready(Err(Self::overflow_error()));
            }
            let pending = this.pending.clone();
            let pending_len = pending.len();
            // Clamp to the remaining allowance like the fresh-frame
            // branch does: an unclamped serve here could over-serve the
            // cap and wrap the u64 remainder (the pending bytes may
            // exceed what is left).
            let max = pending_len.min(this.remaining as usize);
            this.serve(&pending, buf, max);
            return Poll::Ready(Ok(()));
        }
        // Cap exhausted: probe for a further byte before declaring EOF, so
        // an over-cap body is detected even when it lands exactly on the
        // boundary and then keeps going.
        if this.remaining == 0 || this.eof {
            if this.eof {
                return Poll::Ready(Ok(()));
            }
            return match Pin::new(&mut this.body).frame().poll_unpin(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(None) => {
                    this.eof = true;
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Some(Ok(frame))) => {
                    if frame.is_data() && frame.data_ref().is_some_and(|d| !d.is_empty()) {
                        this.exceeded = true;
                        return Poll::Ready(Err(Self::overflow_error()));
                    }
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Some(Err(e))) => Poll::Ready(Err(io::Error::other(e))),
            };
        }
        match Pin::new(&mut this.body).frame().poll_unpin(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => {
                this.eof = true;
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Err(io::Error::other(e))),
            Poll::Ready(Some(Ok(frame))) => match frame.data_ref() {
                Some(data) if !data.is_empty() => {
                    let max = (this.remaining as usize).min(buf.remaining());
                    this.serve(data, buf, max);
                    Poll::Ready(Ok(()))
                }
                _ => Poll::Ready(Ok(())),
            },
        }
    }
}
