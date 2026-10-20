//! One process per library: an exclusive OS lock on `<library>/catalog.lock`, held for as long as
//! the library is open (issue #99).
//!
//! Two processes on the same library (the app and `lightcraft-cli --library`, two app instances,
//! two computers on a shared folder) would each keep their own catalog and `seq` counter, and
//! whichever wrote the last snapshot would silently drop the other's edits. So opening a library
//! takes the lock first, and a second opener gets [`LockError::InUse`] instead.
//!
//! - The lock is the OS's (`flock` on Unix, `LockFileEx` on Windows, via [`std::fs::File::try_lock`]):
//!   it is released explicitly when the [`LibraryLock`] is dropped and, after a crash or a kill, by
//!   the OS when the process ends. A `catalog.lock` file left behind is therefore never "stale":
//!   only a lock that's actually held refuses the open.
//! - Dropping the lock calls [`std::fs::File::unlock`] rather than relying on the handle closing.
//!   On Unix an `flock` lock belongs to the open file description and lasts while *any* descriptor
//!   on it is open; a process that another thread is spawning holds a copy of every descriptor
//!   (`O_CLOEXEC` only closes them at `execve`), so closing ours would leave the library "already
//!   open" for a moment and a re-open would be refused (issue #680). `LOCK_UN` through our
//!   descriptor frees the lock whatever copies a child still holds.
//! - Who holds it (process id, computer, program, version) is written to `catalog.lock.owner`, for
//!   the error message. It's a separate file because a Windows lock also blocks reading the
//!   locked file. It's only a hint: it may be missing or left over from a crash.
//! - If the lock file can't be opened or the file system can't lock it, opening fails. A warning
//!   cannot protect edits from a second writer. Locks over SMB/NFS still depend on the server.
//! - Browser storage (OPFS) has no file locks: see the web host for its own guard.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

/// The lock file in the library directory.
pub const LOCK: &str = "catalog.lock";
/// Who holds the lock (JSON, see [`LockOwner`]).
pub const OWNER: &str = "catalog.lock.owner";

/// Who has the library open (as written in [`OWNER`]).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LockOwner {
    pub pid: u32,
    /// Computer name, if known.
    pub host: String,
    /// The program (`LightCraft`, `lightcraft-cli`…) and its version.
    pub program: String,
    pub version: String,
    /// When it opened the library (seconds since 1970).
    pub since: u64,
}

impl LockOwner {
    /// This process, as `program`.
    pub fn this_process(program: &str) -> LockOwner {
        LockOwner {
            pid: std::process::id(),
            host: host_name(),
            program: program.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            since: web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        }
    }

    /// "LightCraft 0.3.0 (process 123 on studio-mac)".
    pub fn describe(&self) -> String {
        let program = if self.program.is_empty() { "another LightCraft" } else { self.program.as_str() };
        let version = if self.version.is_empty() { String::new() } else { format!(" {}", self.version) };
        let host = if self.host.is_empty() {
            String::new()
        } else if self.host == host_name() {
            " on this computer".to_string()
        } else {
            format!(" on {}", self.host)
        };
        format!("{program}{version} (process {}{host})", self.pid)
    }
}

/// The computer's name, best effort (empty if unknown).
fn host_name() -> String {
    let env = ["COMPUTERNAME", "HOSTNAME"].iter().find_map(|k| std::env::var(k).ok().filter(|h| !h.trim().is_empty()));
    env.or_else(|| std::fs::read_to_string("/etc/hostname").ok()).map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).unwrap_or_default()
}

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// Another process has the library open.
    #[error("This library is already open in {}. Close it there first.", .0.as_ref().map(LockOwner::describe).unwrap_or_else(|| "another LightCraft program".into()))]
    InUse(Option<LockOwner>),
    /// Exclusive access could not be established; the library must not open writable.
    #[error("Cannot safely open this library: can't lock {}: {source}. Check the lock file's permissions and use storage that supports exclusive file locks.", path.display())]
    Unavailable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// The exclusive lock on a library directory; released on drop (and by the OS if the process
/// dies).
#[derive(Debug)]
pub struct LibraryLock {
    dir: PathBuf,
    file: File,
}

impl LibraryLock {
    /// Lock `dir` (which must exist) for this process. Fails unless exclusive access is secured.
    pub fn acquire(dir: &Path, program: &str) -> Result<LibraryLock, LockError> {
        Self::acquire_using(dir, program, File::try_lock)
    }

    fn acquire_using(dir: &Path, program: &str, try_lock: impl FnOnce(&File) -> Result<(), TryLockError>) -> Result<LibraryLock, LockError> {
        let unavailable = |source| LockError::Unavailable { path: dir.join(LOCK), source };
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join(LOCK)).map_err(unavailable)?;
        match try_lock(&file) {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                let owner = std::fs::read(dir.join(OWNER)).ok().and_then(|b| serde_json::from_slice::<LockOwner>(&b).ok());
                return Err(LockError::InUse(owner));
            }
            Err(TryLockError::Error(e)) => return Err(unavailable(e)),
        }
        let owner = LockOwner::this_process(program);
        if let Ok(json) = serde_json::to_vec_pretty(&owner)
            && let Err(e) = std::fs::write(dir.join(OWNER), json)
        {
            log::warn!("library {}: can't write {OWNER}: {e}", dir.display());
        }
        Ok(LibraryLock { dir: dir.to_path_buf(), file })
    }

    /// A successfully acquired library lock always holds the OS lock.
    pub fn held(&self) -> bool {
        true
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Another descriptor on the locked file: what a process spawned by another thread holds
    /// between fork and exec (a `dup` refers to the same open file description). Tests only (the
    /// test that uses it is Unix-only: `flock` semantics).
    #[cfg(all(test, unix))]
    pub(crate) fn descriptor_copy(&self) -> Option<File> {
        self.file.try_clone().ok()
    }
}

impl Drop for LibraryLock {
    /// Remove the owner note while still holding the lock (so it can't delete the next holder's),
    /// then release the lock explicitly. Closing the handle isn't enough on Unix: `flock` lasts
    /// while any copy of the descriptor is open, and a process another thread is spawning holds a
    /// copy of every descriptor until it execs (issue #680). If the unlock fails the handle still
    /// closes right after, which releases the lock once every copy is gone, as before.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.dir.join(OWNER));
        if let Err(e) = self.file.unlock() {
            log::warn!("library {}: can't release {LOCK}: {e}", self.dir.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_lock_errors_refuse_open_and_preserve_the_owner_note() {
        let dir = std::env::temp_dir().join(format!("lc-lock-unsupported-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let note = b"existing owner";
        std::fs::write(dir.join(OWNER), note).unwrap();
        for kind in [io::ErrorKind::Unsupported, io::ErrorKind::PermissionDenied, io::ErrorKind::Other] {
            let error = LibraryLock::acquire_using(&dir, "LightCraft", |_| Err(TryLockError::Error(io::Error::from(kind)))).unwrap_err();
            assert!(matches!(error, LockError::Unavailable { source, .. } if source.kind() == kind));
            assert_eq!(std::fs::read(dir.join(OWNER)).unwrap(), note);
        }
        let held = LibraryLock::acquire(&dir, "LightCraft").unwrap();
        assert!(held.held(), "failed attempts did not retain a descriptor lock");
        drop(held);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
