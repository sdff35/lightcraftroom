use axum::body::Body;
use chrono::Utc;
use lightcraft_cloud::{
    models::Photo,
    storage::{Storage, supported_signature},
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;
fn bytes() -> Vec<u8> {
    let mut b = vec![0xff, 0xd8, 0xff];
    b.extend(0..61);
    b
}
fn photo(data: &[u8]) -> Photo {
    Photo {
        id: Uuid::new_v4(),
        library_id: Uuid::new_v4(),
        object_id: Uuid::new_v4(),
        operation_id: Uuid::new_v4(),
        original_name: "sample.jpg".into(),
        media_type: "image/jpeg".into(),
        byte_length: data.len() as i64,
        sha256: hex::encode(Sha256::digest(data)),
        metadata: serde_json::json!({"rating":3,"title":"original metadata"}),
        status: "pending".into(),
        revision: 1,
        created_at: Utc::now(),
        stored_at: None,
        deleted_at: None,
    }
}
fn path(dir: &TempDir, p: &Photo, extension: &str) -> std::path::PathBuf {
    dir.path().join("originals").join(format!("{}.{}", p.object_id, extension))
}
#[tokio::test]
async fn immutable_original_and_manifest_survive_reopen() {
    let dir = TempDir::new().unwrap();
    let data = bytes();
    let p = photo(&data);
    let storage = Storage::open(dir.path()).unwrap();
    storage.receive(&p, Body::from(data.clone()), 0).await.unwrap();
    storage.verify(&p, true).await.unwrap();
    assert_eq!(std::fs::read(path(&dir, &p, "bin")).unwrap(), data);
    drop(storage);
    let reopened = Storage::open(dir.path()).unwrap();
    reopened.verify(&p, true).await.unwrap();
}
#[tokio::test]
async fn wrong_checksum_short_body_and_oversize_leave_no_original() {
    let dir = TempDir::new().unwrap();
    let data = bytes();
    let p = photo(&data);
    let storage = Storage::open(dir.path()).unwrap();
    for bad in [vec![0u8; data.len()], data[..data.len() - 1].to_vec(), vec![0u8; data.len() + 1]] {
        assert!(storage.receive(&p, Body::from(bad), 0).await.is_err());
        assert!(!path(&dir, &p, "bin").exists());
        assert_eq!(std::fs::read_dir(dir.path().join("staging")).unwrap().count(), 0);
    }
}
#[tokio::test]
async fn original_and_manifest_corruption_are_detected() {
    let dir = TempDir::new().unwrap();
    let data = bytes();
    let p = photo(&data);
    let storage = Storage::open(dir.path()).unwrap();
    storage.receive(&p, Body::from(data.clone()), 0).await.unwrap();
    std::fs::write(path(&dir, &p, "json"), b"{}").unwrap();
    assert_eq!(storage.verify(&p, true).await.unwrap_err().code, "integrity_error");
    storage.write_manifest(&p).await.unwrap();
    let mut changed = data;
    changed[10] ^= 1;
    std::fs::write(path(&dir, &p, "bin"), changed).unwrap();
    assert_eq!(storage.verify(&p, true).await.unwrap_err().code, "integrity_error");
}
#[tokio::test]
async fn replay_cannot_replace_an_existing_original() {
    let dir = TempDir::new().unwrap();
    let data = bytes();
    let p = photo(&data);
    let storage = Storage::open(dir.path()).unwrap();
    storage.receive(&p, Body::from(data.clone()), 0).await.unwrap();
    storage.receive(&p, Body::from(vec![1; 64]), 0).await.unwrap();
    assert_eq!(std::fs::read(path(&dir, &p, "bin")).unwrap(), data);
}
#[tokio::test]
async fn full_disk_guard_and_interrupted_stream_clean_up() {
    let dir = TempDir::new().unwrap();
    let data = bytes();
    let p = photo(&data);
    let storage = Storage::open(dir.path()).unwrap();
    assert!(storage.check_space(u64::MAX, 1).is_err());
    let stream = futures_util::stream::iter([
        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(&[0xff, 0xd8, 0xff])),
        Err(std::io::Error::other("disconnected")),
    ]);
    assert!(storage.receive(&p, Body::from_stream(stream), 0).await.is_err());
    assert_eq!(std::fs::read_dir(dir.path().join("staging")).unwrap().count(), 0);
}
#[test]
fn second_writer_is_refused_and_lock_recovers() {
    let dir = TempDir::new().unwrap();
    let a = Storage::open(dir.path()).unwrap();
    assert!(Storage::open(dir.path()).is_err());
    drop(a);
    assert!(Storage::open(dir.path()).is_ok());
}
#[test]
fn envelope_screening_never_decodes_or_accepts_arbitrary_content() {
    assert!(supported_signature("image/jpeg", &bytes()));
    assert!(supported_signature("application/octet-stream", b"II\x2a\0raw data"));
    assert!(supported_signature("application/octet-stream", b"\0\0\0\x14ftypcrx \0\0\0\0"));
    assert!(!supported_signature("image/jpeg", b"<html>bad</html>"));
    assert!(!supported_signature("application/octet-stream", b"not a RAW file"));
}
#[cfg(unix)]
#[tokio::test]
async fn linked_originals_are_rejected() {
    use std::os::unix::fs::symlink;
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let data = bytes();
    let p = photo(&data);
    let storage = Storage::open(dir.path()).unwrap();
    let target = outside.path().join("original");
    std::fs::write(&target, data).unwrap();
    symlink(&target, path(&dir, &p, "bin")).unwrap();
    assert!(storage.verify(&p, false).await.is_err());
    std::fs::remove_file(path(&dir, &p, "bin")).unwrap();
    std::fs::hard_link(&target, path(&dir, &p, "bin")).unwrap();
    assert!(storage.verify(&p, false).await.is_err());
}
