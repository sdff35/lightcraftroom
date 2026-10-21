use crate::{Client, Error, Photo, Result, Uuid, json, network, status};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Hashing,
    Uploading,
    Downloading,
    Verifying,
}
#[derive(Clone, Debug, Serialize)]
pub struct Progress {
    pub phase: Phase,
    pub done: u64,
    pub total: u64,
    pub name: String,
}
pub type Notify = Arc<dyn Fn(Progress) + Send + Sync>;
struct ProgressReader {
    file: File,
    cancel: Arc<AtomicBool>,
    notify: Notify,
    progress: Progress,
}
impl Read for ProgressReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("cancelled"));
        }
        let count = self.file.read(buf)?;
        self.progress.done = self.progress.done.saturating_add(count as u64);
        (self.notify)(self.progress.clone());
        Ok(count)
    }
}
pub fn media_type(name: &str) -> Result<&'static str> {
    let extension = name.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" => Ok("image/jpeg"),
        "dng" | "cr2" | "cr3" | "nef" | "nrw" | "arw" | "srf" | "sr2" | "raf" | "orf" | "rw2" | "rwl" | "pef" | "srw" | "3fr" | "fff" | "iiq"
        | "mef" | "mos" | "erf" | "kdc" | "dcr" | "raw" => Ok("application/octet-stream"),
        _ => Err(Error::File("Choose a JPEG or a supported camera RAW file.".into())),
    }
}
fn io_error(_: std::io::Error) -> Error {
    Error::File("The file could not be read or written. Check permissions and free disk space.".into())
}
impl Client {
    pub fn upload(&mut self, library: Uuid, path: &Path, operation: Uuid, cancel: Arc<AtomicBool>, notify: Notify) -> Result<Photo> {
        let name = path.file_name().and_then(|s| s.to_str()).ok_or(Error::Protocol)?.to_owned();
        let media = media_type(&name)?;
        let mut file = File::open(path).map_err(io_error)?;
        let meta = file.metadata().map_err(io_error)?;
        let size = meta.len();
        if !meta.is_file() || !(16..=self.config.max_upload_bytes).contains(&size) {
            return Err(Error::File(format!("Choose a regular file between 16 bytes and {} MiB.", self.config.max_upload_bytes / 1048576)));
        }
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        let mut done = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let n = file.read(&mut buffer).map_err(io_error)?;
            if n == 0 {
                break;
            }
            done = done.saturating_add(n as u64);
            if done > size {
                return Err(Error::Integrity);
            }
            hash.update(buffer.get(..n).ok_or(Error::Protocol)?);
            notify(Progress { phase: Phase::Hashing, done, total: size, name: name.clone() });
        }
        if done != size {
            return Err(Error::Integrity);
        }
        let sha256 = format!("{:x}", hash.finalize());
        file.seek(SeekFrom::Start(0)).map_err(io_error)?;
        let access = Zeroizing::new(self.access()?);
        let intent:Photo=json(self.http.post(format!("{}/v1/libraries/{library}/photos",self.server)).bearer_auth(access.as_str()).json(&serde_json::json!({"operation_id":operation,"original_name":name,"media_type":media,"byte_length":size,"sha256":sha256,"metadata":{}})).send().map_err(network)?)?;
        intent.validate()?;
        if intent.sha256 != sha256 || intent.byte_length != size || intent.library_id != library {
            return Err(Error::Protocol);
        }
        let reader = ProgressReader {
            file,
            cancel: cancel.clone(),
            notify: notify.clone(),
            progress: Progress { phase: Phase::Uploading, done: 0, total: size, name: name.clone() },
        };
        let response = self
            .http
            .put(format!("{}/v1/photos/{}/original", self.server, intent.id))
            .timeout(Duration::from_secs(330))
            .bearer_auth(access.as_str())
            .header("Content-Type", media)
            .body(reqwest::blocking::Body::sized(reader, size))
            .send();
        let result = response.map_err(network).and_then(json::<Photo>);
        if result.is_err() {
            // A lost success response can still mean ready. A retry of the same intent is safe.
            if let Ok(existing) = self.get::<Photo>(&format!("/v1/photos/{}", intent.id))
                && existing.status == "ready"
                && existing.sha256 == sha256
            {
                return Ok(existing);
            }
            if cancel.load(Ordering::Relaxed) {
                let _ = self.http.delete(format!("{}/v1/photos/{}/upload", self.server, intent.id)).bearer_auth(access.as_str()).send();
                return Err(Error::Cancelled);
            }
        }
        let ready = result?;
        ready.validate()?;
        if ready.status != "ready" || ready.sha256 != sha256 || ready.byte_length != size {
            return Err(Error::Integrity);
        }
        notify(Progress { phase: Phase::Verifying, done: size, total: size, name });
        Ok(ready)
    }
    pub fn download(&mut self, photo: &Photo, path: &Path, cancel: Arc<AtomicBool>, notify: Notify) -> Result<()> {
        photo.validate()?;
        if photo.status != "ready" {
            return Err(Error::Busy);
        }
        // Never replace a user file, even if the save dialog approved an existing name.
        if path.exists() {
            return Err(Error::File("That file already exists. Choose a new filename to keep the existing copy.".into()));
        }
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        if fs2::available_space(parent).map_err(io_error)? < photo.byte_length.saturating_add(64 * 1024 * 1024) {
            return Err(Error::Quota);
        }
        let mut staging = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
        let access = Zeroizing::new(self.access()?);
        let mut response = status(
            self.http
                .get(format!("{}/v1/photos/{}/original", self.server, photo.id))
                .timeout(Duration::from_secs(330))
                .bearer_auth(access.as_str())
                .send()
                .map_err(network)?,
        )?;
        if response.content_length() != Some(photo.byte_length) {
            return Err(Error::Integrity);
        }
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        let mut done = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let n = response.read(&mut buffer).map_err(|_| Error::Network)?;
            if n == 0 {
                break;
            }
            done = done.saturating_add(n as u64);
            if done > photo.byte_length {
                return Err(Error::Integrity);
            }
            let chunk = buffer.get(..n).ok_or(Error::Protocol)?;
            hash.update(chunk);
            staging.write_all(chunk).map_err(io_error)?;
            notify(Progress { phase: Phase::Downloading, done, total: photo.byte_length, name: photo.original_name.clone() });
        }
        if done != photo.byte_length || format!("{:x}", hash.finalize()) != photo.sha256 {
            return Err(Error::Integrity);
        }
        staging.as_file().sync_all().map_err(io_error)?;
        staging
            .persist_noclobber(path)
            .map_err(|_| Error::File("Could not save the verified file. Check permissions and choose a new filename.".into()))?;
        #[cfg(unix)]
        File::open(parent).and_then(|f| f.sync_all()).map_err(io_error)?;
        notify(Progress { phase: Phase::Verifying, done, total: photo.byte_length, name: photo.original_name.clone() });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_jpeg_and_camera_raw_only() {
        assert_eq!(media_type("photo.NEF").unwrap(), "application/octet-stream");
        assert_eq!(media_type("photo.JPEG").unwrap(), "image/jpeg");
        assert!(media_type("photo.exe").is_err());
    }
}
