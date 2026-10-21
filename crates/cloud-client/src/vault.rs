//! Refresh-token storage. There is deliberately no plaintext file implementation.
use crate::{Error, Result};
pub trait TokenVault: Send + Sync {
    fn load(&self, account: &str) -> Result<Option<String>>;
    fn save(&self, account: &str, token: &str) -> Result<()>;
    fn remove(&self, account: &str) -> Result<()>;
}
#[derive(Default)]
pub struct SystemVault;
impl TokenVault for SystemVault {
    fn load(&self, account: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new("org.lightcraft.cloud", account).map_err(|_| Error::Vault)?;
        match entry.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(Error::Vault),
        }
    }
    fn save(&self, account: &str, token: &str) -> Result<()> {
        keyring::Entry::new("org.lightcraft.cloud", account).map_err(|_| Error::Vault)?.set_password(token).map_err(|_| Error::Vault)
    }
    fn remove(&self, account: &str) -> Result<()> {
        let entry = keyring::Entry::new("org.lightcraft.cloud", account).map_err(|_| Error::Vault)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(Error::Vault),
        }
    }
}
