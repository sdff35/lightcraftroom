use crate::{
    AppState,
    auth::{Principal, authorize},
    db,
    error::{ApiError, Result},
    models::{Library, NewLibrary, NewPhoto, Page, Photo},
};
use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{
        DefaultBodyLimit, Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use uuid::Uuid;

fn uuid(id: &str) -> Result<Uuid> {
    Uuid::parse_str(id).map_err(|_| ApiError::invalid("Invalid resource UUID"))
}
fn input<T>(value: std::result::Result<Json<T>, JsonRejection>) -> Result<T> {
    value.map(|Json(v)| v).map_err(|e| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large", "JSON exceeds 64 KiB")
        } else {
            ApiError::invalid("Expected valid JSON matching the request schema")
        }
    })
}
fn page(value: std::result::Result<Query<Page>, QueryRejection>) -> Result<Page> {
    value.map(|Query(v)| v).map_err(|_| ApiError::invalid("Invalid pagination parameters"))
}
async fn no_route() -> ApiError {
    ApiError::not_found()
}
async fn no_method() -> ApiError {
    ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed", "This method is not supported")
}
pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/v1/me", get(me))
        .route("/v1/libraries", post(create_library).get(libraries))
        .route("/v1/libraries/{id}", get(library))
        .route("/v1/libraries/{id}/photos", post(create_photo).get(photos))
        .route("/v1/photos/{id}", get(photo))
        .route("/v1/photos/{id}/verify", post(verify))
        .route("/v1/photos/{id}/upload", delete(cancel))
        .layer(DefaultBodyLimit::max(65536))
        .merge(Router::new().route("/v1/photos/{id}/original", put(upload).get(download)).layer(DefaultBodyLimit::disable()))
        .route_layer(middleware::from_fn_with_state(state.clone(), authorize));
    Router::new()
        .route("/health/live", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/health/ready", get(ready))
        .route("/v1/config", get(public_config))
        .merge(protected)
        .fallback(no_route)
        .method_not_allowed_fallback(no_method)
        .layer(middleware::from_fn(|request: axum::extract::Request, next: middleware::Next| async move {
            let mut r = next.run(request).await;
            r.headers_mut().insert("x-content-type-options", axum::http::HeaderValue::from_static("nosniff"));
            r.headers_mut().entry(header::CACHE_CONTROL).or_insert(axum::http::HeaderValue::from_static("no-store"));
            r
        }))
        .layer(tower_http::catch_panic::CatchPanicLayer::custom(|_| {
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "The request could not be completed").into_response()
        }))
        .with_state(state)
}
async fn public_config(State(s): State<AppState>) -> Json<Value> {
    Json(json!({"api_version":1,"issuer":s.config.issuer,"client_id":"lightcraft-desktop","max_upload_bytes":s.config.max_upload}))
}
async fn ready(State(s): State<AppState>) -> Result<Json<Value>> {
    sqlx::query("SELECT 1").execute(&s.pool).await?;
    s.storage.free()?;
    s.oidc.ready().await?;
    Ok(Json(json!({"status":"ready"})))
}
async fn me(Extension(p): Extension<Principal>) -> Result<Json<Value>> {
    p.require("lightcraft:read")?;
    Ok(Json(json!({"id":p.user_id,"subject":p.subject,"scope":p.scope})))
}
async fn owned_library(s: &AppState, p: &Principal, id: Uuid) -> Result<Library> {
    sqlx::query_as::<_, Library>("SELECT * FROM libraries WHERE id=$1 AND owner_id=$2")
        .bind(id)
        .bind(p.user_id)
        .fetch_optional(&s.pool)
        .await?
        .ok_or_else(ApiError::not_found)
}
async fn owned_photo(s: &AppState, p: &Principal, id: Uuid) -> Result<Photo> {
    sqlx::query_as::<_, Photo>("SELECT p.* FROM photos p JOIN libraries l ON l.id=p.library_id WHERE p.id=$1 AND l.owner_id=$2")
        .bind(id)
        .bind(p.user_id)
        .fetch_optional(&s.pool)
        .await?
        .ok_or_else(ApiError::not_found)
}
async fn create_library(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    body: std::result::Result<Json<NewLibrary>, JsonRejection>,
) -> Result<(StatusCode, Json<Library>)> {
    p.require("lightcraft:write")?;
    let input = input(body)?;
    let name = input.name.trim();
    if name.is_empty() || name.len() > 200 || name.chars().any(char::is_control) {
        return Err(ApiError::invalid("Invalid library name"));
    }
    let mut tx = s.pool.begin().await?;
    // Serialize each owner's creation and bound empty-library metadata growth.
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE").bind(p.user_id).execute(&mut *tx).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM libraries WHERE owner_id=$1").bind(p.user_id).fetch_one(&mut *tx).await?;
    if count >= 100 {
        return Err(ApiError::conflict("library_limit", "An owner may have at most 100 libraries"));
    }
    let result = sqlx::query_as::<_, Library>("INSERT INTO libraries(id,owner_id,name) VALUES($1,$2,$3) RETURNING *")
        .bind(Uuid::new_v4())
        .bind(p.user_id)
        .bind(name)
        .fetch_one(&mut *tx)
        .await?;
    db::change(&mut tx, result.id, result.id, "library.created", 1).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(result)))
}
async fn libraries(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    query: std::result::Result<Query<Page>, QueryRejection>,
) -> Result<Json<Value>> {
    p.require("lightcraft:read")?;
    let q = page(query)?;
    let limit = q.limit()?;
    let items = sqlx::query_as::<_, Library>("SELECT * FROM libraries WHERE owner_id=$1 AND ($2::uuid IS NULL OR id>$2) ORDER BY id LIMIT $3")
        .bind(p.user_id)
        .bind(q.after)
        .bind(limit)
        .fetch_all(&s.pool)
        .await?;
    let after = if items.len() as i64 == limit { items.last().map(|i| i.id) } else { None };
    Ok(Json(json!({"items":items,"next_after":after})))
}
async fn library(State(s): State<AppState>, Extension(p): Extension<Principal>, Path(id): Path<String>) -> Result<Json<Library>> {
    p.require("lightcraft:read")?;
    Ok(Json(owned_library(&s, &p, uuid(&id)?).await?))
}
async fn create_photo(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
    body: std::result::Result<Json<NewPhoto>, JsonRejection>,
) -> Result<(StatusCode, Json<Photo>)> {
    p.require("lightcraft:write")?;
    let library = uuid(&id)?;
    owned_library(&s, &p, library).await?;
    let input = input(body)?;
    input.validate(s.config.max_upload)?;
    let mut tx = s.pool.begin().await?;
    let budget: (i64, i64, i32) =
        sqlx::query_as("SELECT used_bytes,reserved_bytes,pending_count FROM storage_budget WHERE singleton=true FOR UPDATE")
            .fetch_one(&mut *tx)
            .await?;
    if let Some(old) = sqlx::query_as::<_, Photo>("SELECT * FROM photos WHERE library_id=$1 AND operation_id=$2")
        .bind(library)
        .bind(input.operation_id)
        .fetch_optional(&mut *tx)
        .await?
    {
        return if old.matches(&input) {
            Ok((StatusCode::OK, Json(old)))
        } else {
            Err(ApiError::conflict("idempotency_conflict", "This operation ID was already used with a different request"))
        };
    }
    let owner_pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM photos p JOIN libraries l ON l.id=p.library_id WHERE l.owner_id=$1 AND p.status='pending'")
            .bind(p.user_id)
            .fetch_one(&mut *tx)
            .await?;
    if budget.2 >= 1000 || owner_pending >= 32 {
        return Err(ApiError::conflict("pending_limit", "Cancel or finish existing pending uploads"));
    }
    let charge = input.byte_length.checked_add(db::MANIFEST_RESERVE).ok_or_else(ApiError::storage)?;
    if budget.0.checked_add(budget.1).and_then(|x| x.checked_add(charge)).is_none_or(|x| x > s.config.quota) {
        return Err(ApiError::new(StatusCode::INSUFFICIENT_STORAGE, "quota_exceeded", "The LightCraft storage quota has been reached"));
    }
    s.storage.check_space(u64::try_from(budget.1.saturating_add(charge)).map_err(|_| ApiError::storage())?, s.config.min_free)?;
    sqlx::query("UPDATE storage_budget SET reserved_bytes=reserved_bytes+$1,pending_count=pending_count+1 WHERE singleton=true")
        .bind(charge)
        .execute(&mut *tx)
        .await?;
    let result=sqlx::query_as::<_,Photo>("INSERT INTO photos(id,library_id,object_id,operation_id,original_name,media_type,byte_length,sha256,metadata) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING *")
        .bind(Uuid::new_v4()).bind(library).bind(Uuid::new_v4()).bind(input.operation_id).bind(&input.original_name).bind(&input.media_type).bind(input.byte_length).bind(&input.sha256).bind(serde_json::to_value(input.metadata).map_err(|_|ApiError::invalid("Invalid metadata"))?).fetch_one(&mut *tx).await?;
    db::change(&mut tx, library, result.id, "photo.pending", 1).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(result)))
}
async fn photos(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
    query: std::result::Result<Query<Page>, QueryRejection>,
) -> Result<Json<Value>> {
    p.require("lightcraft:read")?;
    let id = uuid(&id)?;
    owned_library(&s, &p, id).await?;
    let q = page(query)?;
    let limit = q.limit()?;
    let items = sqlx::query_as::<_, Photo>("SELECT * FROM photos WHERE library_id=$1 AND ($2::uuid IS NULL OR id>$2) ORDER BY id LIMIT $3")
        .bind(id)
        .bind(q.after)
        .bind(limit)
        .fetch_all(&s.pool)
        .await?;
    let after = if items.len() as i64 == limit { items.last().map(|i| i.id) } else { None };
    Ok(Json(json!({"items":items,"next_after":after})))
}
async fn photo(State(s): State<AppState>, Extension(p): Extension<Principal>, Path(id): Path<String>) -> Result<Json<Photo>> {
    p.require("lightcraft:read")?;
    Ok(Json(owned_photo(&s, &p, uuid(&id)?).await?))
}
fn busy() -> ApiError {
    ApiError::new(StatusCode::TOO_MANY_REQUESTS, "storage_busy", "The storage concurrency limit has been reached")
}
async fn upload(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<(StatusCode, Json<Photo>)> {
    p.require("lightcraft:write")?;
    let id = uuid(&id)?;
    owned_photo(&s, &p, id).await?;
    let _permit = s.file_ops.clone().try_acquire_owned().map_err(|_| busy())?;
    let length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.parse::<i64>().ok())
        .ok_or_else(|| ApiError::new(StatusCode::LENGTH_REQUIRED, "length_required", "Content-Length is required"))?;
    let mut tx = s.pool.begin().await?;
    let photo = sqlx::query_as::<_, Photo>("SELECT * FROM photos WHERE id=$1 FOR UPDATE NOWAIT").bind(id).fetch_one(&mut *tx).await?;
    if length != photo.byte_length {
        return Err(ApiError::invalid("Content-Length does not match the registered original"));
    }
    if headers.get(header::CONTENT_TYPE).and_then(|h| h.to_str().ok()) != Some(photo.media_type.as_str()) {
        return Err(ApiError::new(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media", "Content-Type does not match the registered original"));
    }
    if photo.status == "ready" {
        s.storage.verify(&photo, true).await?;
        return Ok((StatusCode::OK, Json(photo)));
    }
    if photo.status != "pending" {
        return Err(ApiError::conflict("upload_cancelled", "The upload was cancelled"));
    }
    tokio::time::timeout(s.config.upload_timeout, s.storage.receive(&photo, body, s.config.min_free))
        .await
        .map_err(|_| ApiError::new(StatusCode::REQUEST_TIMEOUT, "upload_timeout", "The upload deadline was reached"))??;
    let result = db::mark_ready(&mut tx, &photo).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(result)))
}
async fn download(State(s): State<AppState>, Extension(p): Extension<Principal>, Path(id): Path<String>) -> Result<Response> {
    p.require("lightcraft:read")?;
    let photo = owned_photo(&s, &p, uuid(&id)?).await?;
    if photo.status != "ready" {
        return Err(ApiError::conflict("not_ready", "The original has not been stored"));
    }
    let permit = s.file_ops.clone().try_acquire_owned().map_err(|_| busy())?;
    let file = s.storage.verify(&photo, true).await?;
    let stream = tokio_util::io::ReaderStream::new(file).map(move |chunk| {
        let _keep = &permit;
        chunk
    });
    let filename = percent_encoding::utf8_percent_encode(&photo.original_name, percent_encoding::NON_ALPHANUMERIC).to_string();
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &photo.media_type)
        .header(header::CONTENT_LENGTH, photo.byte_length.to_string())
        .header(header::ETAG, format!("\"sha256:{}\"", photo.sha256))
        .header("x-content-sha256", &photo.sha256)
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"original\"; filename*=UTF-8''{filename}"))
        .header(header::ACCEPT_RANGES, "none")
        .body(Body::from_stream(stream))
        .map_err(|_| ApiError::unavailable())?;
    Ok(response)
}
async fn verify(State(s): State<AppState>, Extension(p): Extension<Principal>, Path(id): Path<String>) -> Result<Json<Value>> {
    p.require("lightcraft:read")?;
    let photo = owned_photo(&s, &p, uuid(&id)?).await?;
    if photo.status != "ready" {
        return Err(ApiError::conflict("not_ready", "The original has not been stored"));
    }
    let _permit = s.file_ops.clone().try_acquire_owned().map_err(|_| busy())?;
    s.storage.verify(&photo, true).await?;
    Ok(Json(json!({"verified":true,"sha256":photo.sha256,"byte_length":photo.byte_length})))
}
async fn cancel(State(s): State<AppState>, Extension(p): Extension<Principal>, Path(id): Path<String>) -> Result<Json<Photo>> {
    p.require("lightcraft:write")?;
    let id = uuid(&id)?;
    owned_photo(&s, &p, id).await?;
    let mut tx = s.pool.begin().await?;
    let photo = sqlx::query_as::<_, Photo>("SELECT * FROM photos WHERE id=$1 FOR UPDATE NOWAIT").bind(id).fetch_one(&mut *tx).await?;
    if photo.status == "cancelled" {
        return Ok(Json(photo));
    }
    if photo.status == "ready" || s.storage.exists(photo.object_id)? {
        return Err(ApiError::conflict("original_present", "A completed original cannot be cancelled; retry finalization"));
    }
    sqlx::query(
        "UPDATE storage_budget SET reserved_bytes=reserved_bytes-$1,used_bytes=used_bytes+$2,pending_count=pending_count-1 WHERE singleton=true",
    )
    .bind(db::charge(&photo))
    .bind(db::MANIFEST_RESERVE)
    .execute(&mut *tx)
    .await?;
    let result = sqlx::query_as::<_, Photo>("UPDATE photos SET status='cancelled',revision=revision+1,deleted_at=now() WHERE id=$1 RETURNING *")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    db::change(&mut tx, photo.library_id, id, "photo.cancelled", result.revision).await?;
    tx.commit().await?;
    Ok(Json(result))
}
