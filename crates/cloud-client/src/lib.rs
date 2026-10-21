//! Native Cloud protocol client. Blocking operations belong on a worker thread.
//! Refresh tokens live only in the OS credential service; access tokens stay in memory.
#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod auth;
#[cfg(test)]
mod tests;
mod transfer;
pub mod vault;

pub use auth::PendingLogin;
pub use transfer::{Phase, Progress};
pub use uuid::Uuid;

use reqwest::blocking::{Client as Http, Response};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    sync::Arc,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Address,
    Network,
    Timeout,
    Unauthorized,
    Forbidden,
    NotFound,
    Busy,
    Quota,
    Integrity,
    Protocol,
    Cancelled,
    Vault,
    CallbackBusy,
    Browser,
    File(String),
    Server(u16),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Address => {
                "Enter an HTTPS server address without a path, credentials or query. HTTP is allowed only on loopback for local testing."
            }
            Self::Network => "Cannot reach the server. Check the address, network and HTTPS certificate, then retry.",
            Self::Timeout => "The server did not respond in time. Check your connection and retry.",
            Self::Unauthorized => "Your sign-in has expired or was refused. Sign in again in your browser.",
            Self::Forbidden => "This account does not have permission for this operation.",
            Self::NotFound => "This library or photo is unavailable to your account. Refresh the library.",
            Self::Busy => "The server is busy. Wait a moment and retry.",
            Self::Quota => "The storage limit or free-space reserve has been reached.",
            Self::Integrity => "The file failed its size or SHA-256 check. No unverified copy was saved.",
            Self::Protocol => "The server returned an invalid or incompatible Cloud response.",
            Self::Cancelled => "Operation cancelled. An interrupted upload must be retried from the beginning.",
            Self::Vault => "The system credential store is unavailable or locked. Unlock it and retry; no token was written to a settings file.",
            Self::CallbackBusy => "The local sign-in port 47861 is busy. Close another sign-in window and retry.",
            Self::Browser => "The browser could not be opened. Use Open sign-in page to continue.",
            Self::File(message) => return write!(f, "{message}"),
            Self::Server(code) => return write!(f, "Cloud server error ({code}). Retry when the server is available."),
        };
        f.write_str(text)
    }
}
impl std::error::Error for Error {}
fn network(e: reqwest::Error) -> Error {
    if e.is_timeout() { Error::Timeout } else { Error::Network }
}
fn status(response: Response) -> Result<Response> {
    match response.status().as_u16() {
        200..=299 => Ok(response),
        401 => Err(Error::Unauthorized),
        403 => Err(Error::Forbidden),
        404 => Err(Error::NotFound),
        409 | 429 => Err(Error::Busy),
        507 => Err(Error::Quota),
        400 | 411 | 413 | 415 | 422 => Err(Error::Protocol),
        code => Err(Error::Server(code)),
    }
}
fn json<T: DeserializeOwned>(response: Response) -> Result<T> {
    let response = status(response)?;
    if response.content_length().is_some_and(|n| n > 1_048_576) {
        return Err(Error::Protocol);
    }
    let mut bytes = Vec::new();
    response.take(1_048_577).read_to_end(&mut bytes).map_err(|_| Error::Network)?;
    if bytes.len() > 1_048_576 {
        return Err(Error::Protocol);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::Protocol)
}
pub fn server_address(input: &str) -> Result<String> {
    let url = reqwest::Url::parse(input.trim()).map_err(|_| Error::Address)?;
    if !secure_url(&url) || !matches!(url.path(), "" | "/") {
        return Err(Error::Address);
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}
fn secure_url(url: &reqwest::Url) -> bool {
    url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && (url.scheme() == "https" || (url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerConfig {
    pub api_version: u32,
    pub issuer: String,
    pub client_id: String,
    pub max_upload_bytes: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Identity {
    pub id: Uuid,
    pub subject: String,
    pub scope: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Library {
    pub id: Uuid,
    pub name: String,
    pub revision: i64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Photo {
    pub id: Uuid,
    pub library_id: Uuid,
    pub operation_id: Uuid,
    pub original_name: String,
    pub media_type: String,
    pub byte_length: u64,
    pub sha256: String,
    pub metadata: serde_json::Value,
    pub status: String,
    pub revision: i64,
}
impl Photo {
    pub fn validate(&self) -> Result<()> {
        if self.byte_length > 536_870_912
            || self.byte_length < 16
            || self.sha256.len() != 64
            || !self.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.original_name.len() > 255
            || self.original_name.contains(['/', '\\'])
            || self.original_name.chars().any(char::is_control)
            || !matches!(self.status.as_str(), "pending" | "ready" | "cancelled")
        {
            return Err(Error::Protocol);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_after: Option<Uuid>,
}
struct Tokens {
    access: Zeroizing<String>,
    refresh: Zeroizing<String>,
    expires: Instant,
}
pub struct Client {
    pub server: String,
    pub config: ServerConfig,
    pub identity: Option<Identity>,
    pub remembered: bool,
    pub vault_warning: bool,
    http: Http,
    discovery: auth::Discovery,
    tokens: Option<Tokens>,
    vault: Arc<dyn vault::TokenVault>,
    vault_key: String,
    saved_refresh: bool,
}
impl Client {
    pub fn connect(server: &str, vault: Arc<dyn vault::TokenVault>) -> Result<Self> {
        let server = server_address(server)?;
        let _ = rustls_rustcrypto::provider().install_default();
        let http = Http::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .user_agent("LightCraft-Cloud-Desktop/1")
            .build()
            .map_err(network)?;
        status(http.get(format!("{server}/health/ready")).send().map_err(network)?)?;
        let config: ServerConfig = json(http.get(format!("{server}/v1/config")).send().map_err(network)?)?;
        if config.api_version != 1
            || config.client_id != "lightcraft-desktop"
            || config.max_upload_bytes > 536_870_912
            || config.max_upload_bytes < 16
        {
            return Err(Error::Protocol);
        }
        let discovery = auth::Discovery::load(&http, &config.issuer)?;
        let vault_key = format!("{:x}", Sha256::digest(format!("{server}\n{}\n{}", config.issuer, config.client_id)));
        Ok(Self {
            server,
            config,
            identity: None,
            remembered: false,
            vault_warning: false,
            http,
            discovery,
            tokens: None,
            vault,
            vault_key,
            saved_refresh: false,
        })
    }
    pub fn restore(&mut self) -> Result<bool> {
        let refresh = match self.vault.load(&self.vault_key) {
            Ok(Some(v)) => Zeroizing::new(v),
            Ok(None) => return Ok(false),
            Err(_) => {
                self.vault_warning = true;
                return Ok(false);
            }
        };
        self.remembered = true;
        self.saved_refresh = true;
        self.refresh_with(&refresh)?;
        self.identity = Some(self.me()?);
        Ok(true)
    }
    /// Recheck an existing connection without discarding a session-only sign-in.
    pub fn check_connection(&mut self) -> Result<()> {
        status(self.http.get(format!("{}/health/ready", self.server)).send().map_err(network)?)?;
        if self.identity.is_some() {
            self.identity = Some(self.me()?);
        }
        Ok(())
    }
    fn access(&mut self) -> Result<String> {
        let token = self.tokens.as_ref().ok_or(Error::Unauthorized)?;
        if token.expires <= Instant::now() + Duration::from_secs(30) {
            let refresh = Zeroizing::new(token.refresh.to_string());
            self.refresh_with(&refresh)?;
        }
        self.tokens.as_ref().map(|t| t.access.to_string()).ok_or(Error::Unauthorized)
    }
    fn get<T: DeserializeOwned>(&mut self, path: &str) -> Result<T> {
        let access = Zeroizing::new(self.access()?);
        json(self.http.get(format!("{}{}", self.server, path)).bearer_auth(access.as_str()).send().map_err(network)?)
    }
    pub fn me(&mut self) -> Result<Identity> {
        self.get("/v1/me")
    }
    pub fn libraries(&mut self, after: Option<Uuid>) -> Result<Page<Library>> {
        let page: Page<Library> = self.get(&format!("/v1/libraries?limit=100{}", after.map(|a| format!("&after={a}")).unwrap_or_default()))?;
        if page.items.len() > 100 || page.items.iter().any(|l| l.name.len() > 200) {
            return Err(Error::Protocol);
        }
        Ok(page)
    }
    pub fn create_library(&mut self, name: &str) -> Result<Library> {
        let name = name.trim();
        if name.is_empty() || name.len() > 200 || name.chars().any(char::is_control) {
            return Err(Error::File("Enter a library name (1–200 characters).".into()));
        }
        let access = Zeroizing::new(self.access()?);
        json(
            self.http
                .post(format!("{}/v1/libraries", self.server))
                .bearer_auth(access.as_str())
                .json(&serde_json::json!({"name":name}))
                .send()
                .map_err(network)?,
        )
    }
    pub fn photos(&mut self, library: Uuid, after: Option<Uuid>) -> Result<Page<Photo>> {
        let page: Page<Photo> =
            self.get(&format!("/v1/libraries/{library}/photos?limit=100{}", after.map(|a| format!("&after={a}")).unwrap_or_default()))?;
        if page.items.len() > 100 {
            return Err(Error::Protocol);
        }
        for photo in &page.items {
            photo.validate()?;
            if photo.library_id != library {
                return Err(Error::Protocol);
            }
        }
        Ok(page)
    }
}
