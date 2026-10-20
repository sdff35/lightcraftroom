use lightcraft_cloud::{
    config::Config,
    models::{Metadata, NewPhoto},
};
use serde_json::json;
fn input() -> NewPhoto {
    serde_json::from_value(json!({"operation_id":uuid::Uuid::new_v4(),"original_name":"photo.nef","media_type":"application/octet-stream","byte_length":64,"sha256":"a".repeat(64)})).unwrap()
}
#[test]
fn metadata_and_original_names_are_bounded() {
    let p = input();
    assert!(p.validate(1024).is_ok());
    for name in ["../photo.nef", "a\\photo.nef", "photo.html", "photo.nef\n"] {
        let mut p = input();
        p.original_name = name.into();
        assert!(p.validate(1024).is_err());
    }
    for length in [-1, 0, 15, 1025, i64::MAX] {
        let mut p = input();
        p.byte_length = length;
        assert!(p.validate(1024).is_err());
    }
    let mut p = input();
    p.sha256 = "A".repeat(64);
    assert!(p.validate(1024).is_err());
    let mut m = Metadata { rating: 6, ..Default::default() };
    assert!(m.validate().is_err());
    m.rating = 0;
    m.width = Some(0);
    assert!(m.validate().is_err());
    assert!(serde_json::from_value::<NewPhoto>(json!({"unexpected":1})).is_err());
}
#[test]
fn config_requires_https_and_explicit_development_override() {
    let c = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        storage: "/tmp/not-opened".into(),
        issuer: "https://auth.example/realms/lightcraft".into(),
        audience: "lightcraft-cloud".into(),
        jwks_url: "https://auth.example/keys".into(),
        allow_http_oidc: false,
        allow_http_jwks: false,
        max_upload: 1024,
        quota: 10000,
        min_free: 0,
        upload_timeout: std::time::Duration::from_secs(30),
        db_host: "localhost".into(),
        db_port: 5432,
        db_name: "unused".into(),
        db_user: "unused".into(),
        db_password_file: "/not-read".into(),
    };
    assert!(c.validate().is_ok());
    let mut internal = c.clone();
    internal.jwks_url = "http://lc-auth:8080/realms/lightcraft/protocol/openid-connect/certs".into();
    assert!(internal.validate().is_err());
    internal.allow_http_jwks = true;
    assert!(internal.validate().is_ok());
    internal.jwks_url = "http://external.example/keys".into();
    assert!(internal.validate().is_err());
    let mut local = c.clone();
    local.issuer = "http://127.0.0.1:18881/realms/lightcraft".into();
    assert!(local.validate().is_err());
    local.allow_http_oidc = true;
    assert!(local.validate().is_ok());
    local.jwks_url = "http://user:password@host/keys".into();
    assert!(local.validate().is_err());
}
