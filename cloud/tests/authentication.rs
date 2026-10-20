use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, encode};
use lightcraft_cloud::auth::{Oidc, Principal, validate_token};
use rsa::{
    RsaPrivateKey,
    pkcs1::{EncodeRsaPrivateKey, EncodeRsaPublicKey},
    traits::PublicKeyParts,
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
struct Keys {
    encoding: EncodingKey,
    decoding: DecodingKey,
    jwks: Value,
}
fn keys() -> &'static Keys {
    static KEYS: OnceLock<Keys> = OnceLock::new();
    KEYS.get_or_init(||{
        let private=RsaPrivateKey::new(&mut rand::thread_rng(),2048).unwrap();let public=private.to_public_key();
        Keys {encoding:EncodingKey::from_rsa_der(private.to_pkcs1_der().unwrap().as_bytes()),decoding:DecodingKey::from_rsa_der(public.to_pkcs1_der().unwrap().as_bytes()),jwks:json!({"keys":[{"kty":"RSA","kid":"test-key","alg":"RS256","use":"sig","n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]})}
    })
}
fn claims() -> Value {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    json!({"sub":"subject-a","iss":"https://auth.example/realms/lightcraft","aud":"lightcraft-cloud","iat":now,"exp":now+300,"typ":"Bearer","scope":"lightcraft:read lightcraft:write"})
}
fn sign(c: &Value) -> String {
    let mut h = Header::new(Algorithm::RS256);
    h.kid = Some("test-key".into());
    encode(&h, c, &keys().encoding).unwrap()
}
fn verify(token: &str) -> lightcraft_cloud::error::Result<lightcraft_cloud::auth::Claims> {
    validate_token(token, &keys().decoding, "https://auth.example/realms/lightcraft", "lightcraft-cloud")
}
#[test]
fn valid_token_and_exact_scopes() {
    let result = verify(&sign(&claims())).unwrap();
    let p = Principal { user_id: uuid::Uuid::new_v4(), subject: result.sub, scope: result.scope };
    assert!(p.require("lightcraft:read").is_ok());
    assert!(p.require("lightcraft:write").is_ok());
    assert_eq!(p.require("lightcraft:rea").unwrap_err().status, 403);
}
#[test]
fn rejected_claims_and_missing_fields() {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    for (name, value) in [
        ("iss", json!("https://evil.example")),
        ("aud", json!("other-api")),
        ("typ", json!("ID")),
        ("sub", json!("")),
        ("exp", json!(now - 60)),
        ("nbf", json!(now + 600)),
        ("iat", json!(now + 60)),
    ] {
        let mut c = claims();
        c[name] = value;
        assert!(verify(&sign(&c)).is_err(), "accepted invalid {name}");
    }
    for name in ["iss", "aud", "exp", "sub", "iat", "typ"] {
        let mut c = claims();
        c.as_object_mut().unwrap().remove(name);
        assert!(verify(&sign(&c)).is_err(), "accepted missing {name}");
    }
}
#[test]
fn reject_tampered_signature_and_hmac_algorithm() {
    let mut token = sign(&claims()).into_bytes();
    let start = token.iter().rposition(|b| *b == b'.').unwrap() + 2;
    token[start] = if token[start] == b'a' { b'b' } else { b'a' };
    assert!(verify(std::str::from_utf8(&token).unwrap()).is_err());
    let token = encode(&Header::new(Algorithm::HS256), &claims(), &EncodingKey::from_secret(&rand::random::<[u8; 32]>())).unwrap();
    assert!(verify(&token).is_err());
}
#[tokio::test]
async fn jwks_cache_and_unknown_kid_are_bounded_and_do_not_follow_token_urls() {
    let _ = rustls_rustcrypto::provider().install_default();
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = calls.clone();
    let jwks = keys().jwks.clone();
    let router = axum::Router::new().route(
        "/keys",
        axum::routing::get(move || {
            let calls = handler_calls.clone();
            let jwks = jwks.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                axum::Json(jwks)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let oidc = Oidc::new("https://auth.example/realms/lightcraft".into(), "lightcraft-cloud".into(), format!("http://{addr}/keys")).unwrap();
    oidc.ready().await.unwrap();
    for _ in 0..4 {
        assert!(oidc.verify(&sign(&claims())).await.is_ok());
    }
    let mut h = Header::new(Algorithm::RS256);
    h.kid = Some("unknown".into());
    h.jku = Some("https://untrusted.invalid/keys".into());
    let token = encode(&h, &claims(), &keys().encoding).unwrap();
    for _ in 0..4 {
        assert_eq!(oidc.verify(&token).await.unwrap_err().status, 401);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.abort();
    assert!(oidc.verify(&sign(&claims())).await.is_ok(), "existing keys remain usable only within their bounded TTL");
}
