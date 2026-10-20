#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod models;
pub mod storage;
use config::Config;
use error::Result;
use std::sync::Arc;
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: sqlx::PgPool,
    pub oidc: auth::Oidc,
    pub storage: storage::Storage,
    pub file_ops: Arc<tokio::sync::Semaphore>,
}
pub async fn reconcile(pool: &sqlx::PgPool, storage: &storage::Storage) -> Result<()> {
    storage.clean_staging().await?;
    // Every pending object has durable intent in PostgreSQL before any bytes can be written.
    // A completed file survives a crash between fsync and DB commit, and is finalized here.
    let pending = sqlx::query_as::<_, models::Photo>("SELECT * FROM photos WHERE status='pending' ORDER BY id LIMIT 1001").fetch_all(pool).await?;
    if pending.len() > 1000 {
        return Err(error::ApiError::unavailable());
    }
    for photo in pending {
        if storage.exists(photo.object_id)? {
            let mut tx = pool.begin().await?;
            let photo = sqlx::query_as::<_, models::Photo>("SELECT * FROM photos WHERE id=$1 AND status='pending' FOR UPDATE")
                .bind(photo.id)
                .fetch_one(&mut *tx)
                .await?;
            storage.verify(&photo, false).await?;
            storage.write_manifest(&photo).await?;
            db::mark_ready(&mut tx, &photo).await?;
            tx.commit().await?;
        }
    }
    Ok(())
}
