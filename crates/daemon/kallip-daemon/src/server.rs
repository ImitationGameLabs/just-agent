//! UDS accept loop: one short connection, one request line, one response
//! line. Every handler is answered from a fresh directory scan; the daemon
//! keeps no in-memory registry (the tree is the truth).

use std::path::PathBuf;

use kallip_daemon_common::wire::{
    ErrorCode, InstanceState, MAX_LINE_BYTES, OkPayload, PROTOCOL_VERSION, RequestBody, Response,
    decode_request, encode_response, err, ok,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt as _, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

use crate::scan;

/// Shared handler context for one daemon process.
#[derive(Debug, Clone)]
pub struct Daemon {
    pub data_root: PathBuf,
}

impl Daemon {
    pub fn new(data_root: PathBuf) -> Self {
        Self { data_root }
    }

    /// Serve requests on `listener` until the process is stopped.
    pub async fn serve(self, listener: UnixListener) -> anyhow::Result<()> {
        loop {
            let (stream, _) = listener.accept().await?;
            // SO_PEERCRED: the kernel answers who is on the other end.
            // The 0600 socket already gates access; the uid becomes a
            // spawned instance's owner so a multi-user install can account
            // instances per requesting peer.
            let peer_uid = match stream.peer_cred() {
                Ok(cred) => cred.uid(),
                Err(error) => {
                    tracing::warn!(%error, "peer credentials unavailable");
                    continue;
                }
            };
            let daemon = self.clone();
            tokio::spawn(async move {
                if let Err(error) = daemon.handle(stream, peer_uid).await {
                    tracing::warn!(%error, "connection handler failed");
                }
            });
        }
    }

    async fn handle(&self, stream: tokio::net::UnixStream, peer_uid: u32) -> anyhow::Result<()> {
        let (reader, mut writer) = stream.into_split();
        let reader = BufReader::new(reader);
        // Cap the request line at the protocol limit: `take` bounds the
        // read, so a runaway client streaming bytes cannot grow memory
        // unbounded — an over-long line reads back truncated (no newline)
        // and fails to parse, which answers bad_request.
        let mut reader = reader.take(MAX_LINE_BYTES as u64 + 1);
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok(()); // connected and left: nothing to answer
        }
        let response = match decode_request(line.trim_end()) {
            Ok(request) if request.v == PROTOCOL_VERSION => {
                self.dispatch(request.body, peer_uid).await
            }
            Ok(_) => err(ErrorCode::BadRequest, "unsupported protocol version"),
            Err(error) => err(
                ErrorCode::BadRequest,
                format!("unparseable request: {error}"),
            ),
        };
        let mut out = encode_response(&response)?.into_bytes();
        out.push(b'\n');
        writer.write_all(&out).await?;
        writer.flush().await?;
        Ok(())
    }

    async fn dispatch(&self, body: RequestBody, peer_uid: u32) -> Response {
        let instances = scan::scan_instances(&self.data_root);
        match body {
            RequestBody::List => ok(OkPayload::List {
                instances: instances.iter().map(|i| i.info()).collect(),
            }),
            RequestBody::Health { slug: None } => ok(OkPayload::Health {
                report: kallip_daemon_common::wire::HealthReport {
                    slug: None,
                    running: true,
                    state: InstanceState::Running,
                    detail: None,
                },
            }),
            RequestBody::Health { slug: Some(slug) } => {
                match instances.iter().find(|i| i.slug == slug) {
                    Some(instance) => ok(OkPayload::Health {
                        report: instance.health(),
                    }),
                    None => err(ErrorCode::NotFound, format!("no instance named {slug}")),
                }
            }
            RequestBody::Spawn {
                slug,
                workspace,
                env,
            } => {
                let slug_out = slug.clone();
                // Blocking work on the connection task: spawn waits up to
                // 30s for the pidfile; spawn_blocking keeps the runtime free.
                let timeout = std::time::Duration::from_secs(30);
                match tokio::task::spawn_blocking({
                    let data_root = self.data_root.clone();
                    move || {
                        let slug = slug.clone();
                        crate::spawn::spawn(&data_root, &slug, &workspace, &env, timeout, peer_uid)
                    }
                })
                .await
                {
                    Ok(Ok((pid, port))) => ok(OkPayload::Spawn {
                        slug: slug_out,
                        pid,
                        port,
                    }),
                    Ok(Err(error)) => spawn_error_response(&error),
                    Err(join_error) => {
                        err(ErrorCode::Internal, format!("spawn task: {join_error}"))
                    }
                }
            }
            RequestBody::Stop { slug } => {
                let slug_out = slug.clone();
                match tokio::task::spawn_blocking({
                    let data_root = self.data_root.clone();
                    move || crate::stop::stop(&data_root, &slug, &crate::scan::pid_is_tagma)
                })
                .await
                {
                    Ok(Ok(())) => ok(OkPayload::Stop { slug: slug_out }),
                    Ok(Err(error)) => {
                        let code = kallip_daemon_common::wire::ErrorCode::from(&error);
                        err(code, error.to_string())
                    }
                    Err(join_error) => err(ErrorCode::Internal, format!("stop task: {join_error}")),
                }
            }
        }
    }
}

fn spawn_error_response(error: &crate::spawn::SpawnError) -> Response {
    use crate::spawn::SpawnError;
    let code = match error {
        SpawnError::SlugTaken(_) => ErrorCode::SlugTaken,
        SpawnError::Overlap { .. } => ErrorCode::WorkspaceOverlap,
        SpawnError::Invalid(_) => ErrorCode::InvalidSpawnInput,
        SpawnError::Timeout { .. } => ErrorCode::SpawnTimeout,
        SpawnError::Internal(_) => ErrorCode::Internal,
    };
    err(code, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kallip_daemon_client::DaemonClient;

    #[tokio::test]
    async fn uds_round_trip_list_and_health() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("alpha")).expect("instance dir");
        std::fs::write(
            dir.path().join("alpha/meta.json"),
            r#"{"instance_id":"id-1","owner_uid":1000,"workspace":"/tmp/w"}"#,
        )
        .expect("meta");
        let socket = dir.path().join("control.sock");
        let listener = UnixListener::bind(&socket).expect("bind");

        let daemon = Daemon::new(dir.path().to_path_buf());
        let task = tokio::spawn(daemon.serve(listener));

        let client = DaemonClient::new(&socket);
        let list = client.call(RequestBody::List).await.expect("list");
        match list.body {
            kallip_daemon_common::wire::ResponseBody::Ok { payload } => {
                let kallip_daemon_common::wire::OkPayload::List { instances } = payload else {
                    panic!("expected list payload");
                };
                assert_eq!(instances.len(), 1);
                assert_eq!(instances[0].slug, "alpha");
                assert_eq!(instances[0].workspace, "/tmp/w");
                assert_eq!(instances[0].state, InstanceState::Stopped);
            }
            other => panic!("expected ok, got {other:?}"),
        }

        let health = client
            .call(RequestBody::Health {
                slug: Some("alpha".into()),
            })
            .await
            .expect("health");
        match health.body {
            kallip_daemon_common::wire::ResponseBody::Ok {
                payload: kallip_daemon_common::wire::OkPayload::Health { report },
            } => {
                assert_eq!(report.state, InstanceState::Stopped);
            }
            other => panic!("expected ok, got {other:?}"),
        }

        let missing = client
            .call(RequestBody::Health {
                slug: Some("nope".into()),
            })
            .await
            .expect("health missing");
        match missing.body {
            kallip_daemon_common::wire::ResponseBody::Err { code, .. } => {
                assert_eq!(code, ErrorCode::NotFound);
            }
            other => panic!("expected err, got {other:?}"),
        }

        // A deliberately bad line gets bad_request, not a dropped connection.
        let response = client
            .raw_line("{\"v\":1,\"type\":\"nonsense\"}")
            .await
            .expect("connection still answers");
        match response.body {
            kallip_daemon_common::wire::ResponseBody::Err { code, .. } => {
                assert_eq!(code, ErrorCode::BadRequest);
            }
            other => panic!("expected bad request, got {other:?}"),
        }

        task.abort();
    }
}
