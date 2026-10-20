use crate::{error::Result, models::Photo};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub const MANIFEST_RESERVE: i64 = 65536;
pub fn charge(photo: &Photo) -> i64 {
    photo.byte_length.saturating_add(MANIFEST_RESERVE)
}
pub async fn change(tx: &mut Transaction<'_, Postgres>, library: Uuid, entity: Uuid, kind: &str, revision: i64) -> Result<()> {
    let seq: i64 = sqlx::query_scalar("UPDATE libraries SET change_seq=change_seq+1 WHERE id=$1 RETURNING change_seq")
        .bind(library)
        .fetch_one(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO changes(library_id,seq,entity_id,kind,revision,payload) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(library)
        .bind(seq)
        .bind(entity)
        .bind(kind)
        .bind(revision)
        .bind(serde_json::json!({"entity_id":entity,"revision":revision}))
        .execute(&mut **tx)
        .await?;
    Ok(())
}
pub async fn mark_ready(tx: &mut Transaction<'_, Postgres>, photo: &Photo) -> Result<Photo> {
    sqlx::query(
        "UPDATE storage_budget SET used_bytes=used_bytes+$1,reserved_bytes=reserved_bytes-$1,pending_count=pending_count-1 WHERE singleton=true",
    )
    .bind(charge(photo))
    .execute(&mut **tx)
    .await?;
    let result = sqlx::query_as::<_, Photo>(
        "UPDATE photos SET status='ready',revision=revision+1,stored_at=now() WHERE id=$1 AND status='pending' RETURNING *",
    )
    .bind(photo.id)
    .fetch_one(&mut **tx)
    .await?;
    change(tx, photo.library_id, photo.id, "photo.ready", result.revision).await?;
    Ok(result)
}
