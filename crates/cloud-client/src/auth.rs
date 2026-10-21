use crate::{Client, Error, Http, Result, Tokens, json, network, secure_url, status};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const REDIRECT: &str = "http://127.0.0.1:47861/callback";
#[derive(Deserialize)]
pub(crate) struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    end_session_endpoint: String,
}
impl Discovery {
    pub(crate) fn load(http: &Http, issuer: &str) -> Result<Self> {
        let base = reqwest::Url::parse(issuer).map_err(|_| Error::Protocol)?;
        if !secure_url(&base) || issuer.ends_with('/') {
            return Err(Error::Protocol);
        }
        let d: Self = json(http.get(format!("{issuer}/.well-known/openid-configuration")).send().map_err(network)?)?;
        if d.issuer != issuer {
            return Err(Error::Protocol);
        }
        for endpoint in [&d.authorization_endpoint, &d.token_endpoint, &d.jwks_uri, &d.end_session_endpoint] {
            let u = reqwest::Url::parse(endpoint).map_err(|_| Error::Protocol)?;
            if !secure_url(&u) || u.origin() != base.origin() || !u.path().starts_with(&format!("{}/", base.path())) {
                return Err(Error::Protocol);
            }
        }
        Ok(d)
    }
}
pub struct PendingLogin {
    listener: TcpListener,
    url: String,
    state: String,
    nonce: String,
    verifier: Zeroizing<String>,
    issuer: String,
}
fn random() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| Error::Protocol)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
impl PendingLogin {
    pub fn url(&self) -> &str {
        &self.url
    }
    fn callback(&self, line: &str) -> Result<String> {
        let mut parts = line.split_whitespace();
        if parts.next() != Some("GET") {
            return Err(Error::Protocol);
        }
        let target = parts.next().ok_or(Error::Protocol)?;
        if !target.starts_with("/callback?") || target.len() > 8192 {
            return Err(Error::Protocol);
        }
        let parsed = reqwest::Url::parse(&format!("http://127.0.0.1:47861{target}")).map_err(|_| Error::Protocol)?;
        let pairs: Vec<_> = parsed.query_pairs().collect();
        let single = |key: &str| -> Result<String> {
            let values: Vec<_> = pairs.iter().filter(|(k, _)| k == key).collect();
            if values.len() != 1 {
                return Err(Error::Unauthorized);
            }
            values.first().map(|(_, v)| v.to_string()).ok_or(Error::Unauthorized)
        };
        if single("state")? != self.state {
            return Err(Error::Unauthorized);
        }
        if pairs.iter().any(|(k, _)| k == "iss") && single("iss")? != self.issuer {
            return Err(Error::Unauthorized);
        }
        if pairs.iter().any(|(k, _)| k == "error") {
            return Err(Error::Cancelled);
        }
        let code = single("code")?;
        if code.is_empty() || code.len() > 4096 {
            return Err(Error::Unauthorized);
        }
        Ok(code)
    }
    fn wait(&self, cancel: &AtomicBool) -> Result<String> {
        let deadline = Instant::now() + Duration::from_secs(300);
        while Instant::now() < deadline {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            match self.listener.accept() {
                Ok((mut socket, peer)) => {
                    if !peer.ip().is_loopback() {
                        continue;
                    }
                    socket.set_read_timeout(Some(Duration::from_secs(2))).map_err(|_| Error::Network)?;
                    socket.set_write_timeout(Some(Duration::from_secs(2))).map_err(|_| Error::Network)?;
                    let mut bytes = Vec::new();
                    let mut buf = [0u8; 1024];
                    while bytes.len() < 8192 && !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => bytes.extend_from_slice(buf.get(..n).ok_or(Error::Protocol)?),
                        }
                    }
                    let request = std::str::from_utf8(&bytes).unwrap_or_default();
                    let host_ok = request
                        .lines()
                        .skip(1)
                        .any(|line| line.split_once(':').is_some_and(|(k, v)| k.eq_ignore_ascii_case("host") && v.trim() == "127.0.0.1:47861"));
                    let result = if host_ok { self.callback(request.lines().next().unwrap_or_default()) } else { Err(Error::Protocol) };
                    let (code, text) = if result.is_ok() {
                        ("200 OK", "Sign-in received. You can close this tab and return to LightCraft.")
                    } else {
                        ("400 Bad Request", "This sign-in response was not accepted. Return to LightCraft and try again.")
                    };
                    let _ = write!(
                        socket,
                        "HTTP/1.1 {code}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{text}",
                        text.len()
                    );
                    match result {
                        Ok(code) => return Ok(code),
                        Err(Error::Cancelled) => return Err(Error::Cancelled),
                        _ => {}
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(40)),
                Err(_) => return Err(Error::Network),
            }
        }
        Err(Error::Timeout)
    }
}
#[derive(Deserialize)]
struct TokenReply {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    token_type: String,
    id_token: Option<String>,
}
#[derive(Deserialize)]
struct IdClaims {
    sub: String,
    nonce: String,
    at_hash: Option<String>,
    azp: Option<String>,
}
impl Client {
    pub fn begin_login(&self) -> Result<PendingLogin> {
        let listener = TcpListener::bind("127.0.0.1:47861").map_err(|_| Error::CallbackBusy)?;
        listener.set_nonblocking(true).map_err(|_| Error::Network)?;
        let state = random()?;
        let nonce = random()?;
        let verifier = Zeroizing::new(random()?);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut url = reqwest::Url::parse(&self.discovery.authorization_endpoint).map_err(|_| Error::Protocol)?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", self.config.client_id.as_str()),
            ("response_type", "code"),
            ("redirect_uri", REDIRECT),
            ("scope", "openid"),
            ("state", state.as_str()),
            ("nonce", nonce.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("prompt", "login"),
        ]);
        Ok(PendingLogin { listener, url: url.to_string(), state, nonce, verifier, issuer: self.config.issuer.clone() })
    }
    pub fn finish_login(&mut self, login: PendingLogin, cancel: &AtomicBool, remember: bool) -> Result<()> {
        let code = Zeroizing::new(login.wait(cancel)?);
        let response = self
            .http
            .post(&self.discovery.token_endpoint)
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", self.config.client_id.as_str()),
                ("redirect_uri", REDIRECT),
                ("code", code.as_str()),
                ("code_verifier", login.verifier.as_str()),
            ])
            .send()
            .map_err(network)?;
        let reply: TokenReply = json(response)?;
        let id = reply.id_token.as_deref().ok_or(Error::Unauthorized)?;
        let header = decode_header(id).map_err(|_| Error::Unauthorized)?;
        if header.alg != Algorithm::RS256 {
            return Err(Error::Unauthorized);
        }
        let kid = header.kid.ok_or(Error::Unauthorized)?;
        let keys: JwkSet = json(self.http.get(&self.discovery.jwks_uri).send().map_err(network)?)?;
        if keys.keys.len() > 32 {
            return Err(Error::Protocol);
        }
        let key = keys.find(&kid).ok_or(Error::Unauthorized)?;
        let key = DecodingKey::from_jwk(key).map_err(|_| Error::Unauthorized)?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[self.config.client_id.as_str()]);
        validation.set_issuer(&[self.config.issuer.as_str()]);
        validation.set_required_spec_claims(&["exp", "iss", "sub", "aud", "iat"]);
        validation.validate_nbf = true;
        validation.leeway = 30;
        let claims = decode::<IdClaims>(id, &key, &validation).map_err(|_| Error::Unauthorized)?.claims;
        if claims.nonce != login.nonce || claims.sub.is_empty() || claims.azp.as_ref().is_some_and(|a| a != &self.config.client_id) {
            return Err(Error::Unauthorized);
        }
        if let Some(hash) = claims.at_hash {
            let digest = Sha256::digest(reply.access_token.as_bytes());
            if hash != URL_SAFE_NO_PAD.encode(digest.get(..16).ok_or(Error::Unauthorized)?) {
                return Err(Error::Unauthorized);
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        self.accept(reply)?;
        let me = match self.me() {
            Ok(me) if me.subject == claims.sub => me,
            _ => {
                self.tokens = None;
                return Err(Error::Unauthorized);
            }
        };
        self.identity = Some(me);
        self.remembered = remember;
        if remember {
            self.persist();
        } else {
            match self.vault.remove(&self.vault_key) {
                Ok(()) => self.saved_refresh = false,
                Err(e) if self.saved_refresh => return Err(e),
                Err(_) => self.vault_warning = true,
            }
        }
        Ok(())
    }
    fn accept(&mut self, reply: TokenReply) -> Result<()> {
        if reply.token_type != "Bearer"
            || !(30..=3600).contains(&reply.expires_in)
            || reply.access_token.len() > 16384
            || reply.refresh_token.len() > 32768
            || reply.refresh_token.is_empty()
        {
            return Err(Error::Protocol);
        }
        self.tokens = Some(Tokens {
            access: Zeroizing::new(reply.access_token),
            refresh: Zeroizing::new(reply.refresh_token),
            expires: Instant::now() + Duration::from_secs(reply.expires_in),
        });
        Ok(())
    }
    fn persist(&mut self) {
        if let Some(tokens) = &self.tokens {
            if self.vault.save(&self.vault_key, &tokens.refresh).is_err() {
                self.vault_warning = true;
                self.remembered = false;
            } else {
                self.saved_refresh = true;
                self.vault_warning = false;
            }
        }
    }
    pub(crate) fn refresh_with(&mut self, refresh: &str) -> Result<()> {
        let response = self
            .http
            .post(&self.discovery.token_endpoint)
            .form(&[("grant_type", "refresh_token"), ("client_id", self.config.client_id.as_str()), ("refresh_token", refresh)])
            .send()
            .map_err(network)?;
        if matches!(response.status().as_u16(), 400 | 401) {
            self.tokens = None;
            self.identity = None;
            self.vault.remove(&self.vault_key)?;
            self.saved_refresh = false;
            self.remembered = false;
            return Err(Error::Unauthorized);
        }
        let reply: TokenReply = json(response)?;
        self.accept(reply)?;
        if self.remembered {
            self.persist();
        }
        Ok(())
    }
    pub fn logout(&mut self) -> Result<()> {
        // Do not claim logout while a saved refresh token remains in the credential store.
        let removal = self.vault.remove(&self.vault_key);
        if self.saved_refresh && removal.is_err() {
            return Err(Error::Vault);
        }
        if removal.is_ok() {
            self.saved_refresh = false;
        }
        self.identity = None;
        self.remembered = false;
        let Some(tokens) = self.tokens.take() else {
            return removal;
        };
        status(
            self.http
                .post(&self.discovery.end_session_endpoint)
                .form(&[("client_id", self.config.client_id.as_str()), ("refresh_token", tokens.refresh.as_str())])
                .send()
                .map_err(network)?,
        )?;
        removal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_requires_state_and_rejects_duplicate_or_foreign_issuer() {
        let p = PendingLogin {
            listener: TcpListener::bind("127.0.0.1:0").unwrap(),
            url: String::new(),
            state: "expected".into(),
            nonce: String::new(),
            verifier: Zeroizing::new(String::new()),
            issuer: "https://auth.example/realms/lightcraft".into(),
        };
        assert_eq!(p.callback("GET /callback?code=ok&state=expected HTTP/1.1").unwrap(), "ok");
        for line in [
            "GET /callback?code=ok&state=wrong HTTP/1.1",
            "GET /callback?code=ok&state=expected&state=expected HTTP/1.1",
            "GET /callback?code=ok&state=expected&iss=https%3A%2F%2Fevil.example HTTP/1.1",
            "POST /callback?code=ok&state=expected HTTP/1.1",
        ] {
            assert!(p.callback(line).is_err());
        }
    }
}
