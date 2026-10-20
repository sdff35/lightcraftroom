use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: &'static str,
}
pub type Result<T> = std::result::Result<T, ApiError>;
impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self { status, code, message }
    }
    pub fn invalid(message: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }
    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", "A valid access token is required")
    }
    pub fn unavailable() -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", "The service is temporarily unavailable")
    }
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "The resource was not found")
    }
    pub fn conflict(code: &'static str, message: &'static str) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }
    pub fn storage() -> Self {
        Self::new(StatusCode::INSUFFICIENT_STORAGE, "storage_unavailable", "Storage is unavailable or its safety reserve has been reached")
    }
    pub fn corrupt() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "integrity_error", "The stored original failed integrity checks")
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(json!({"error":{"code":self.code,"message":self.message}}))).into_response();
        response.headers_mut().insert("cache-control", axum::http::HeaderValue::from_static("no-store"));
        if self.status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert("www-authenticate", axum::http::HeaderValue::from_static("Bearer"));
        }
        if self.status == StatusCode::TOO_MANY_REQUESTS || self.status == StatusCode::SERVICE_UNAVAILABLE {
            response.headers_mut().insert("retry-after", axum::http::HeaderValue::from_static("5"));
        }
        response
    }
}
impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        if let sqlx::Error::Database(ref db) = error
            && db.code().as_deref() == Some("55P03")
        {
            return Self::conflict("upload_busy", "An upload is already in progress");
        }
        tracing::error!(context="database operation", error=%error);
        Self::unavailable()
    }
}
impl From<std::io::Error> for ApiError {
    fn from(error: std::io::Error) -> Self {
        tracing::error!(context="storage operation",error=%error);
        Self::storage()
    }
}
