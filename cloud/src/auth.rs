use crate::{
    AppState,
    error::{ApiError, Result},
};
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;

const TTL: Duration = Duration::from_secs(300);
const REFRESH_THROTTLE: Duration = Duration::from_secs(10);
#[derive(Clone)]
pub struct Oidc {
    inner: Arc<Inner>,
}
struct Inner {
    client: reqwest::Client,
    issuer: String,
    audience: String,
    url: String,
    cache: RwLock<Cache>,
}
struct Cache {
    keys: HashMap<String, DecodingKey>,
    fetched: Option<Instant>,
    attempted: Option<Instant>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub iss: String,
    pub exp: u64,
    pub iat: u64,
    pub typ: String,
    #[serde(default)]
    pub scope: String,
}
#[derive(Clone)]
pub struct Principal {
    pub user_id: uuid::Uuid,
    pub subject: String,
    pub scope: String,
}
impl Principal {
    pub fn require(&self, scope: &str) -> Result<()> {
        if self.scope.split_ascii_whitespace().any(|s| s == scope) {
            Ok(())
        } else {
            Err(ApiError::new(axum::http::StatusCode::FORBIDDEN, "insufficient_scope", "The access token does not grant this operation"))
        }
    }
}
impl Oidc {
    pub fn new(issuer: String, audience: String, url: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| ApiError::unavailable())?;
        Ok(Self {
            inner: Arc::new(Inner {
                client,
                issuer,
                audience,
                url,
                cache: RwLock::new(Cache { keys: HashMap::new(), fetched: None, attempted: None }),
            }),
        })
    }
    async fn refresh_locked(&self, cache: &mut Cache) -> Result<()> {
        cache.attempted = Some(Instant::now());
        let response = self.inner.client.get(&self.inner.url).send().await.map_err(|_| ApiError::unavailable())?;
        if !response.status().is_success() || response.content_length().is_some_and(|n| n > 1_048_576) {
            return Err(ApiError::unavailable());
        }
        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ApiError::unavailable())?;
            if bytes.len().saturating_add(chunk.len()) > 1_048_576 {
                return Err(ApiError::unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        let jwks: JwkSet = serde_json::from_slice(&bytes).map_err(|_| ApiError::unavailable())?;
        if jwks.keys.len() > 32 {
            return Err(ApiError::unavailable());
        }
        let mut keys = HashMap::new();
        for jwk in jwks.keys {
            // Only pinned RS256 keys. Never follow jku/x5u supplied by a token.
            let value = serde_json::to_value(&jwk).map_err(|_| ApiError::unavailable())?;
            if value.get("kty").and_then(|x| x.as_str()) != Some("RSA")
                || value.get("use").and_then(|x| x.as_str()).is_some_and(|u| u != "sig")
                || value.get("alg").and_then(|x| x.as_str()).is_some_and(|a| a != "RS256")
            {
                continue;
            }
            let Some(kid) = jwk.common.key_id.as_ref().filter(|k| !k.is_empty() && k.len() <= 128) else { continue };
            let key = DecodingKey::from_jwk(&jwk).map_err(|_| ApiError::unavailable())?;
            if keys.insert(kid.clone(), key).is_some() {
                return Err(ApiError::unavailable());
            }
        }
        if keys.is_empty() {
            return Err(ApiError::unavailable());
        }
        cache.keys = keys;
        cache.fetched = Some(Instant::now());
        Ok(())
    }
    pub async fn ready(&self) -> Result<()> {
        let mut cache = self.inner.cache.write().await;
        if cache.fetched.is_some_and(|t| t.elapsed() < TTL) {
            return Ok(());
        }
        if cache.attempted.is_some_and(|t| t.elapsed() < REFRESH_THROTTLE) {
            return Err(ApiError::unavailable());
        }
        self.refresh_locked(&mut cache).await
    }
    pub async fn verify(&self, token: &str) -> Result<Claims> {
        if token.len() > 16384 {
            return Err(ApiError::unauthorized());
        }
        let header = decode_header(token).map_err(|_| ApiError::unauthorized())?;
        if header.alg != Algorithm::RS256 {
            return Err(ApiError::unauthorized());
        }
        let kid = header.kid.as_deref().filter(|k| !k.is_empty() && k.len() <= 128).ok_or_else(ApiError::unauthorized)?;
        let mut cache = self.inner.cache.write().await;
        let stale = cache.fetched.is_none_or(|t| t.elapsed() >= TTL);
        if stale || !cache.keys.contains_key(kid) {
            if cache.attempted.is_none_or(|t| t.elapsed() >= REFRESH_THROTTLE) {
                self.refresh_locked(&mut cache).await?;
            } else if stale {
                return Err(ApiError::unavailable());
            }
        }
        let key = cache.keys.get(kid).ok_or_else(ApiError::unauthorized)?.clone();
        drop(cache);
        validate_token(token, &key, &self.inner.issuer, &self.inner.audience)
    }
}
pub fn validate_token(token: &str, key: &DecodingKey, issuer: &str, audience: &str) -> Result<Claims> {
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[issuer]);
    validation.set_audience(&[audience]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub", "iat"]);
    validation.validate_nbf = true;
    validation.leeway = 30;
    let claims = decode::<Claims>(token, key, &validation).map_err(|_| ApiError::unauthorized())?.claims;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| ApiError::unavailable())?.as_secs();
    if claims.sub.is_empty()
        || claims.sub.len() > 255
        || claims.typ != "Bearer"
        || claims.iat > now.saturating_add(30)
        || claims.exp <= claims.iat
        || claims.exp.saturating_sub(claims.iat) > 3600
        || claims.scope.len() > 4096
    {
        return Err(ApiError::unauthorized());
    }
    Ok(claims)
}
pub async fn authorize(State(state): State<AppState>, mut request: Request, next: Next) -> Result<Response> {
    let header = request.headers().get("authorization").and_then(|h| h.to_str().ok()).ok_or_else(ApiError::unauthorized)?;
    let token = header.strip_prefix("Bearer ").filter(|t| !t.contains(char::is_whitespace)).ok_or_else(ApiError::unauthorized)?;
    let claims = state.oidc.verify(token).await?;
    let user_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users(id,issuer,subject) VALUES($1,$2,$3) ON CONFLICT(issuer,subject) DO UPDATE SET subject=EXCLUDED.subject RETURNING id",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(&claims.iss)
    .bind(&claims.sub)
    .fetch_one(&state.pool)
    .await?;
    request.extensions_mut().insert(Principal { user_id, subject: claims.sub, scope: claims.scope });
    Ok(next.run(request).await)
}
