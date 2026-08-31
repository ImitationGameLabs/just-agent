//! Reference-counted writes over the metadata store.
//!
//! Invariants this module maintains: a blob's `refcount` equals the number
//! of `file_records` rows pointing at it, and a count never goes
//! negative. [`register_upload`] and [`remove_record`] each move both
//! sides inside one transaction. A row whose count reaches zero is
//! stamped with `freed_at` and left in place; the GC (`crate::gc`)
//! reclaims it -- and then unlinks the blob file -- only after the
//! zero state has held past the configured grace period, which gives a
//! same-content re-upload racing the removal the whole window to land.

use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DbErr, Statement, TransactionTrait,
};
use time::OffsetDateTime;
use uuid::Uuid;

use super::Db;
use crate::blob::BlobId;
use crate::metadata::models::file_records;

/// Outcome of removing a file record.
#[derive(Debug)]
pub struct Removal {
    /// The removed record's blob.
    pub blob_id: BlobId,
    /// True when the removal dropped the refcount to zero: the blob file
    /// has no references left and becomes sweep-eligible (the catalog row
    /// remains at zero until [`crate::gc::sweep`] reclaims it). Purely
    /// informational; no caller action is required.
    pub blob_freed: bool,
}

/// Record one upload: bump the blob's refcount (creating the catalog row on
/// first sight) and insert the file record, atomically. Returns the new
/// record id.
///
/// The refcount bump is one raw upsert-and-return: it reads the pre-update
/// value (`refcount = blob_rows.refcount + 1`), clears any stale
/// `freed_at`, and returns the new count in a single round trip. The
/// conflict-aware insert API can express the self-referential increment,
/// but as an update without the same-query return, so the raw form wins
/// on clarity and round trips. Concurrent uploads of the same blob
/// serialize on the row lock and each leave the count one higher.
pub async fn register_upload(
    db: &Db,
    blob_id: &BlobId,
    size: i64,
    space_path: &str,
    owner: &str,
    provenance: Option<&str>,
) -> Result<Uuid, DbErr> {
    let txn = db.begin().await?;
    let backend = txn.get_database_backend();
    let now = OffsetDateTime::now_utc();

    txn.query_one(Statement::from_sql_and_values(
        backend,
        "INSERT INTO blob_rows (blob_id, size, refcount, created_at) \
         VALUES ($1, $2, 1, $3) \
         ON CONFLICT (blob_id) DO UPDATE SET refcount = blob_rows.refcount + 1, freed_at = NULL \
         RETURNING refcount",
        [blob_id.as_str().into(), size.into(), now.into()],
    ))
    .await?
    .ok_or_else(|| DbErr::Custom("refcount upsert returned no row".to_owned()))?;

    let record_id = Uuid::new_v4();
    file_records::ActiveModel {
        id: Set(record_id),
        space_path: Set(space_path.to_owned()),
        owner: Set(owner.to_owned()),
        blob_id: Set(blob_id.as_str().to_owned()),
        provenance: Set(provenance.map(str::to_owned)),
        created_at: Set(now),
    }
    .insert(&txn)
    .await?;

    txn.commit().await?;
    Ok(record_id)
}

/// Append one delivery event to the log.
pub async fn record_delivery(
    db: &Db,
    from_principal: &str,
    to_principal: &str,
    blob_id: &BlobId,
    source_record_id: Option<Uuid>,
    target_record_id: Option<Uuid>,
) -> Result<Uuid, DbErr> {
    let event = crate::metadata::models::delivery_events::ActiveModel {
        id: Set(Uuid::new_v4()),
        happened_at: Set(OffsetDateTime::now_utc()),
        from_principal: Set(from_principal.to_owned()),
        to_principal: Set(to_principal.to_owned()),
        blob_id: Set(blob_id.as_str().to_owned()),
        source_record_id: Set(source_record_id),
        target_record_id: Set(target_record_id),
    }
    .insert(db)
    .await?;
    Ok(event.id)
}

/// Remove a file record and release its blob reference, atomically.
///
/// Returns `Ok(None)` when the record does not exist (nothing changed; the
/// caller decides whether that is an error). When the release drops the
/// refcount to zero, [`Removal::blob_freed`] is set and the row is
/// stamped with `freed_at`: reclaim-eligible once the zero state holds
/// past the GC grace period. A same-content re-upload landing in the
/// meantime re-registers the row, clearing `freed_at` -- which is what
/// keeps the racing removal from ever taking the file away.
pub async fn remove_record(db: &Db, record_id: Uuid) -> Result<Option<Removal>, DbErr> {
    let txn = db.begin().await?;
    let backend = txn.get_database_backend();

    let deleted = txn
        .query_one(Statement::from_sql_and_values(
            backend,
            "DELETE FROM file_records WHERE id = $1 RETURNING blob_id",
            [record_id.into()],
        ))
        .await?;
    let Some(blob) = deleted.and_then(|row| row.try_get::<String>("", "blob_id").ok()) else {
        // Unknown record: dropping the transaction leaves nothing changed.
        return Ok(None);
    };

    // The `refcount > 0` guard makes a drifted state (record removed,
    // but no catalog row to decrement) harmless rather than a negative
    // count; rows are never deleted here, only by the GC sweep.
    let freed = txn
        .query_one(Statement::from_sql_and_values(
            backend,
            "UPDATE blob_rows SET refcount = refcount - 1, \
             freed_at = CASE WHEN refcount = 1 THEN $2 ELSE freed_at END \
             WHERE blob_id = $1 AND refcount > 0 RETURNING refcount",
            [blob.as_str().into(), OffsetDateTime::now_utc().into()],
        ))
        .await?
        .and_then(|row| row.try_get::<i32>("", "refcount").ok())
        .map(|refcount| refcount == 0)
        .unwrap_or(false);

    txn.commit().await?;
    let blob_id =
        BlobId::parse(&blob).map_err(|e| DbErr::Custom(format!("malformed blob id: {e}")))?;
    Ok(Some(Removal {
        blob_id,
        blob_freed: freed,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::migrated_test_db;
    use sea_orm::EntityTrait;

    fn blob(hex_tail: &str) -> BlobId {
        BlobId::parse(&format!("sha256-{hex_tail}")).expect("valid blob id")
    }

    #[tokio::test]
    async fn register_then_remove_round_trips_refcounts() {
        let db = migrated_test_db().await;
        let id = blob(&"ab".repeat(32));

        let first = register_upload(&db, &id, 5, "/t/a", "alice", Some("from T1"))
            .await
            .expect("first register");
        let second = register_upload(&db, &id, 5, "/t/b", "bob", None)
            .await
            .expect("second register");
        assert_ne!(first, second);

        let out = remove_record(&db, first)
            .await
            .expect("remove")
            .expect("exists");
        assert_eq!(out.blob_id, id);
        assert!(!out.blob_freed, "one reference left, row must stay");

        let out = remove_record(&db, second)
            .await
            .expect("remove")
            .expect("exists");
        assert!(out.blob_freed, "last reference marks the blob reclaimable");

        assert!(
            remove_record(&db, first).await.expect("remove").is_none(),
            "removing an already-removed record reports None"
        );

        let row = crate::metadata::models::blob_rows::Entity::find_by_id(id.as_str())
            .one(&db)
            .await
            .expect("query")
            .expect("catalog row still present");
        assert_eq!(row.refcount, 0, "row stays at zero until the sweep");
        assert!(
            row.freed_at.is_some(),
            "the zero state is stamped as the grace-window start"
        );
    }

    #[tokio::test]
    async fn concurrent_registrations_serialize_to_exact_refcount() {
        let db = std::sync::Arc::new(migrated_test_db().await);
        let id = blob(&"cd".repeat(32));

        // Eight racing uploads of the same blob: the single-statement upsert
        // serializes on the row lock, so no increment can be lost.
        let mut tasks = Vec::new();
        for n in 0..8 {
            let db = db.clone();
            let id = id.clone();
            tasks.push(tokio::spawn(async move {
                register_upload(&db, &id, 1, &format!("/t/{n}"), "owner", None).await
            }));
        }
        for task in tasks {
            task.await.expect("join").expect("register");
        }

        let row = crate::metadata::models::blob_rows::Entity::find_by_id(id.as_str())
            .one(&*db)
            .await
            .expect("query")
            .expect("catalog row exists");
        assert_eq!(row.refcount, 8);
        assert_eq!(row.size, 1);

        // Draining all eight records: exactly one release must observe the
        // drop to zero. `find()` order is unspecified, so count the freed
        // flags instead of assuming which space path goes last.
        let records = crate::metadata::models::file_records::Entity::find()
            .all(&*db)
            .await
            .expect("records");
        let mut freed_count = 0;
        for record in records {
            let out = remove_record(&db, record.id)
                .await
                .expect("remove")
                .expect("exists");
            freed_count += u32::from(out.blob_freed);
        }
        assert_eq!(freed_count, 1, "exactly the final release frees the blob");
        let row = crate::metadata::models::blob_rows::Entity::find_by_id(id.as_str())
            .one(&*db)
            .await
            .expect("query")
            .expect("catalog row still present");
        assert_eq!(
            row.refcount, 0,
            "drained row stays until the GC sweep reclaims it"
        );
    }

    #[tokio::test]
    async fn record_delivery_appends_to_the_log() {
        let db = migrated_test_db().await;
        let id = blob(&"ef".repeat(32));

        let event = record_delivery(&db, "alice", "bob", &id, None, None)
            .await
            .expect("record delivery");
        let rows = crate::metadata::models::delivery_events::Entity::find()
            .all(&db)
            .await
            .expect("events");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, event);
        assert_eq!(rows[0].from_principal, "alice");
        assert_eq!(rows[0].blob_id, id.as_str());
    }
}
