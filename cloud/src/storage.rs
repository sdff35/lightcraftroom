use crate::{
    error::{ApiError, Result},
    models::Photo,
};
use axum::body::Body;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncWriteExt},
};
use uuid::Uuid;

#[derive(Clone)]
pub struct Storage {
    root: Arc<PathBuf>,
    _lock: Arc<std::fs::File>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    photo_id: Uuid,
    library_id: Uuid,
    object_id: Uuid,
    original_name: String,
    media_type: String,
    byte_length: i64,
    sha256: String,
    metadata: serde_json::Value,
}
impl Manifest {
    fn from(photo: &Photo) -> Self {
        Self {
            schema: 1,
            photo_id: photo.id,
            library_id: photo.library_id,
            object_id: photo.object_id,
            original_name: photo.original_name.clone(),
            media_type: photo.media_type.clone(),
            byte_length: photo.byte_length,
            sha256: photo.sha256.clone(),
            metadata: photo.metadata.clone(),
        }
    }
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn read_regular(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    let file = options.open(path)?;
    check_regular(&file)?;
    Ok(file)
}
fn check_regular(file: &std::fs::File) -> Result<()> {
    let info = file.metadata()?;
    if !info.is_file() {
        return Err(ApiError::corrupt());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if info.nlink() != 1 {
            return Err(ApiError::corrupt());
        }
    }
    Ok(())
}
fn directory(path: &Path) -> Result<()> {
    if !std::fs::symlink_metadata(path)?.is_dir() {
        return Err(ApiError::storage());
    }
    Ok(())
}
async fn sync_dir(path: PathBuf) -> Result<()> {
    tokio::task::spawn_blocking(move || std::fs::File::open(path)?.sync_all()).await.map_err(|_| ApiError::storage())??;
    Ok(())
}
impl Storage {
    pub fn open(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        directory(root)?;
        let root = std::fs::canonicalize(root)?;
        for name in ["staging", "originals"] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir)?;
            directory(&dir)?;
        }
        let path = root.join(".writer.lock");
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        }
        let lock = options.open(path)?;
        check_regular(&lock)?;
        std::fs::File::open(&root)?.sync_all()?;
        fs2::FileExt::try_lock_exclusive(&lock)
            .map_err(|_| ApiError::conflict("storage_in_use", "Only one API writer may use this storage volume"))?;
        Ok(Self { root: Arc::new(root), _lock: Arc::new(lock) })
    }
    pub fn free(&self) -> Result<u64> {
        Ok(fs2::available_space(self.root.as_ref())?)
    }
    pub fn check_space(&self, reserved: u64, min_free: u64) -> Result<()> {
        if self.free()? < reserved.saturating_add(min_free) { Err(ApiError::storage()) } else { Ok(()) }
    }
    fn object_path(&self, id: Uuid) -> PathBuf {
        self.root.join("originals").join(format!("{id}.bin"))
    }
    fn manifest_path(&self, id: Uuid) -> PathBuf {
        self.root.join("originals").join(format!("{id}.json"))
    }
    fn temp(&self, id: Uuid) -> Temporary {
        Temporary(self.root.join("staging").join(format!("{id}.{}.part", Uuid::new_v4())))
    }
    pub fn exists(&self, id: Uuid) -> Result<bool> {
        match std::fs::symlink_metadata(self.object_path(id)) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
    pub async fn write_manifest(&self, photo: &Photo) -> Result<()> {
        let bytes = serde_json::to_vec(&Manifest::from(photo)).map_err(|_| ApiError::storage())?;
        if bytes.len() > 16384 {
            return Err(ApiError::invalid("Manifest is too large"));
        }
        let temp = self.temp(photo.object_id);
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temp.0).await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        drop(file);
        let target = self.manifest_path(photo.object_id);
        // Only the API owns this volume and its per-object transaction; originals are never replaced.
        fs::rename(&temp.0, &target).await?;
        sync_dir(self.root.join("originals")).await?;
        Ok(())
    }
    pub async fn receive(&self, photo: &Photo, body: Body, min_free: u64) -> Result<()> {
        if self.exists(photo.object_id)? {
            self.verify(photo, false).await?;
            self.write_manifest(photo).await?;
            return Ok(());
        }
        let temp = self.temp(photo.object_id);
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temp.0).await?;
        let mut stream = body.into_data_stream();
        let mut length = 0u64;
        let mut hasher = Sha256::new();
        let mut prefix = Vec::new();
        while let Some(chunk) = tokio::time::timeout(std::time::Duration::from_secs(30), stream.next())
            .await
            .map_err(|_| ApiError::new(axum::http::StatusCode::REQUEST_TIMEOUT, "upload_timeout", "Upload stopped sending data"))?
        {
            let chunk = chunk.map_err(|_| ApiError::invalid("The upload was interrupted"))?;
            length = length.checked_add(chunk.len() as u64).ok_or_else(|| ApiError::invalid("Upload is too large"))?;
            if length > u64::try_from(photo.byte_length).map_err(|_| ApiError::corrupt())? {
                return Err(ApiError::invalid("Upload exceeds its declared length"));
            }
            self.check_space(chunk.len() as u64, min_free)?;
            if prefix.len() < 64 {
                prefix.extend(chunk.iter().take(64 - prefix.len()));
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
        }
        if length != u64::try_from(photo.byte_length).map_err(|_| ApiError::corrupt())? {
            return Err(ApiError::invalid("Upload length does not match"));
        }
        if hex::encode(hasher.finalize()) != photo.sha256 {
            return Err(ApiError::new(axum::http::StatusCode::UNPROCESSABLE_ENTITY, "checksum_mismatch", "The uploaded SHA-256 does not match"));
        }
        if !supported_signature(&photo.media_type, &prefix) {
            return Err(ApiError::new(
                axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media",
                "Expected a JPEG or a supported RAW container signature",
            ));
        }
        file.sync_all().await?;
        drop(file);
        if self.exists(photo.object_id)? {
            return Err(ApiError::conflict("object_exists", "The immutable object already exists"));
        }
        fs::rename(&temp.0, self.object_path(photo.object_id)).await?;
        sync_dir(self.root.join("originals")).await?;
        self.write_manifest(photo).await
    }
    pub async fn verify(&self, photo: &Photo, manifest: bool) -> Result<File> {
        let path = self.object_path(photo.object_id);
        let mut file = File::from_std(read_regular(&path).map_err(|_| ApiError::corrupt())?);
        if file.metadata().await?.len() != u64::try_from(photo.byte_length).map_err(|_| ApiError::corrupt())? {
            return Err(ApiError::corrupt());
        }
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 65536];
        let mut total = 0u64;
        loop {
            let n = file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            total = total.checked_add(n as u64).ok_or_else(ApiError::corrupt)?;
            if total > photo.byte_length as u64 {
                return Err(ApiError::corrupt());
            }
            let bytes = buf.get(..n).ok_or_else(ApiError::corrupt)?;
            hasher.update(bytes);
        }
        if total != photo.byte_length as u64 || hex::encode(hasher.finalize()) != photo.sha256 {
            return Err(ApiError::corrupt());
        }
        if manifest {
            let path = self.manifest_path(photo.object_id);
            let file = File::from_std(read_regular(&path).map_err(|_| ApiError::corrupt())?);
            let mut bytes = Vec::new();
            file.take(16385).read_to_end(&mut bytes).await?;
            if bytes.len() > 16384 {
                return Err(ApiError::corrupt());
            }
            let actual: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| ApiError::corrupt())?;
            let expected = serde_json::to_value(Manifest::from(photo)).map_err(|_| ApiError::corrupt())?;
            if actual != expected {
                return Err(ApiError::corrupt());
            }
        }
        use tokio::io::AsyncSeekExt;
        file.rewind().await?;
        Ok(file)
    }
    pub async fn clean_staging(&self) -> Result<()> {
        let mut entries = fs::read_dir(self.root.join("staging")).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                return Err(ApiError::storage());
            }
            fs::remove_file(entry.path()).await?;
        }
        sync_dir(self.root.join("staging")).await
    }
}
/// Envelope screening only. Camera decoding and preview generation are worker responsibilities.
pub fn supported_signature(media: &str, bytes: &[u8]) -> bool {
    if media == "image/jpeg" {
        return bytes.starts_with(&[0xff, 0xd8, 0xff]);
    }
    bytes.starts_with(b"II\x2a\0")
        || bytes.starts_with(b"MM\0\x2a")
        || bytes.starts_with(b"FUJIFILMCCD-RAW")
        || bytes.starts_with(b"IIRO")
        || bytes.starts_with(b"IIRS")
        || bytes.starts_with(b"MMOR")
        || bytes.starts_with(b"IIU\0")
        || (bytes.get(4..8) == Some(b"ftyp") && bytes.get(8..12) == Some(b"crx "))
}
