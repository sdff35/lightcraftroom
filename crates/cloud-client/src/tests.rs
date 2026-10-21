use super::*;
use std::{
    io::Write,
    net::TcpListener,
    sync::{Mutex, atomic::AtomicBool},
};

#[derive(Default)]
struct MemoryVault(Mutex<Option<String>>);
impl vault::TokenVault for MemoryVault {
    fn load(&self, _: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, _: &str, token: &str) -> Result<()> {
        *self.0.lock().unwrap() = Some(token.into());
        Ok(())
    }
    fn remove(&self, _: &str) -> Result<()> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}
struct LockedVault;
impl vault::TokenVault for LockedVault {
    fn load(&self, _: &str) -> Result<Option<String>> {
        Err(Error::Vault)
    }
    fn save(&self, _: &str, _: &str) -> Result<()> {
        Err(Error::Vault)
    }
    fn remove(&self, _: &str) -> Result<()> {
        Err(Error::Vault)
    }
}
fn server(tail: Vec<(u16, Vec<u8>, String)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let origin = base.clone();
    let handle = std::thread::spawn(move || {
        let issuer = format!("{origin}/realms/lightcraft");
        let config = serde_json::json!({"api_version":1,"issuer":issuer,"client_id":"lightcraft-desktop","max_upload_bytes":536870912});
        let discovery = serde_json::json!({"issuer":issuer,"authorization_endpoint":format!("{issuer}/auth"),"token_endpoint":format!("{issuer}/token"),"jwks_uri":format!("{issuer}/certs"),"end_session_endpoint":format!("{issuer}/logout")});
        let mut replies = vec![
            (200, b"{}".to_vec(), String::new()),
            (200, config.to_string().into_bytes(), String::new()),
            (200, discovery.to_string().into_bytes(), String::new()),
        ];
        replies.extend(tail);
        let mut requests = Vec::new();
        for (status, body, headers) in replies {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = socket.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                request.extend(&buf[..n]);
            }
            requests.push(String::from_utf8_lossy(&request).into_owned());
            write!(socket, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n", body.len()).unwrap();
            socket.write_all(&body).unwrap();
        }
        requests
    });
    (base, handle)
}
fn authenticated(base: &str) -> Client {
    let mut c = Client::connect(base, Arc::new(MemoryVault::default())).unwrap();
    c.tokens = Some(Tokens {
        access: Zeroizing::new("test-access".into()),
        refresh: Zeroizing::new("test-refresh".into()),
        expires: Instant::now() + Duration::from_secs(300),
    });
    c
}
fn photo(bytes: &[u8]) -> Photo {
    Photo {
        id: Uuid::new_v4(),
        library_id: Uuid::new_v4(),
        operation_id: Uuid::new_v4(),
        original_name: "original.jpg".into(),
        media_type: "image/jpeg".into(),
        byte_length: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
        metadata: serde_json::json!({}),
        status: "ready".into(),
        revision: 2,
    }
}
#[test]
fn addresses_require_https_and_never_accept_embedded_secrets_or_paths() {
    assert_eq!(server_address(" https://photos.example.com/ ").unwrap(), "https://photos.example.com");
    assert!(server_address("http://127.0.0.1:18880").is_ok());
    for url in [
        "http://photos.example.com",
        "https://u:p@example.com",
        "https://example.com/api",
        "https://example.com/?token=x",
        "https://example.com/#x",
        "file:///tmp/photo",
    ] {
        assert_eq!(server_address(url), Err(Error::Address));
    }
}
#[test]
fn downloads_verify_bytes_and_do_not_overwrite_existing_files() {
    let bytes = b"\xff\xd8\xfftest-original-file\xff\xd9";
    let (base, server) = server(vec![(200, bytes.to_vec(), String::new())]);
    let mut c = authenticated(&base);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("original.jpg");
    c.download(&photo(bytes), &dest, Arc::new(AtomicBool::new(false)), Arc::new(|_| {})).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), bytes);
    assert!(matches!(c.download(&photo(bytes), &dest, Arc::new(AtomicBool::new(false)), Arc::new(|_| {})), Err(Error::File(_))));
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests.last().unwrap().to_ascii_lowercase().contains("authorization: bearer test-access"));
}
#[test]
fn corrupt_download_is_removed_and_redirects_never_receive_the_bearer_token() {
    let expected = b"12345678901234567890";
    for (code, body, headers, error) in [
        (200, b"broken-original-data".to_vec(), String::new(), Error::Integrity),
        (302, Vec::new(), "Location: http://127.0.0.1:9/leak\r\n".into(), Error::Server(302)),
        (401, Vec::new(), String::new(), Error::Unauthorized),
    ] {
        let (base, server) = server(vec![(code, body, headers)]);
        let mut c = authenticated(&base);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("original.jpg");
        assert_eq!(c.download(&photo(expected), &dest, Arc::new(AtomicBool::new(false)), Arc::new(|_| {})).unwrap_err(), error);
        assert!(!dest.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        server.join().unwrap();
    }
}
#[test]
fn checking_connection_preserves_a_session_only_sign_in() {
    let id = Uuid::new_v4();
    let me = serde_json::json!({"id":id,"subject":"test-user","scope":"openid"});
    let (base, server) = server(vec![(200, b"{}".to_vec(), String::new()), (200, me.to_string().into_bytes(), String::new())]);
    let mut c = authenticated(&base);
    c.identity = Some(Identity { id, subject: "test-user".into(), scope: "openid".into() });
    c.check_connection().unwrap();
    assert_eq!(c.identity.as_ref().unwrap().id, id);
    assert!(!c.remembered);
    assert_eq!(c.tokens.as_ref().unwrap().access.as_str(), "test-access");
    assert_eq!(server.join().unwrap().len(), 5);
}

#[test]
fn locked_credential_service_never_falls_back_to_plaintext_storage() {
    let (base, server) = server(Vec::new());
    let mut c = Client::connect(&base, Arc::new(LockedVault)).unwrap();
    assert!(!c.restore().unwrap());
    assert!(c.vault_warning);
    assert!(!c.remembered);
    assert!(c.tokens.is_none());
    assert_eq!(c.logout(), Err(Error::Vault), "an inaccessible previous credential must never be silently reported as removed");
    assert!(c.identity.is_none());
    server.join().unwrap();
}
