use crate::error::{ApiError, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub title: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub captured_at: Option<DateTime<Utc>>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    #[serde(default)]
    pub rating: u8,
    #[serde(default)]
    pub tags: Vec<String>,
}
impl Metadata {
    pub fn validate(&self) -> Result<()> {
        if [&self.title, &self.camera_make, &self.camera_model].into_iter().flatten().any(|x| x.len() > 512 || x.chars().any(char::is_control))
            || self.rating > 5
            || self.tags.len() > 32
            || self.tags.iter().any(|x| x.is_empty() || x.len() > 128 || x.chars().any(char::is_control))
            || [self.width, self.height].into_iter().flatten().any(|x| x == 0 || x > 1_000_000)
        {
            return Err(ApiError::invalid("Invalid photo metadata"));
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewLibrary {
    pub name: String,
}
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Library {
    pub id: Uuid,
    pub name: String,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewPhoto {
    pub operation_id: Uuid,
    pub original_name: String,
    pub media_type: String,
    pub byte_length: i64,
    pub sha256: String,
    #[serde(default)]
    pub metadata: Metadata,
}
impl NewPhoto {
    pub fn validate(&self, max: u64) -> Result<()> {
        self.metadata.validate()?;
        let extension = self.original_name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let jpeg = matches!(extension.as_str(), "jpg" | "jpeg");
        let raw = matches!(
            extension.as_str(),
            "dng"
                | "cr2"
                | "cr3"
                | "nef"
                | "nrw"
                | "arw"
                | "srf"
                | "sr2"
                | "raf"
                | "orf"
                | "rw2"
                | "rwl"
                | "pef"
                | "srw"
                | "3fr"
                | "fff"
                | "iiq"
                | "mef"
                | "mos"
                | "erf"
                | "kdc"
                | "dcr"
                | "raw"
        );
        if self.operation_id.is_nil()
            || self.original_name.trim().is_empty()
            || self.original_name.len() > 255
            || self.original_name.contains(['/', '\\'])
            || self.original_name.chars().any(char::is_control)
            || !(jpeg || raw)
            || (jpeg && self.media_type != "image/jpeg")
            || (raw && self.media_type != "application/octet-stream")
            || self.byte_length < 16
            || u64::try_from(self.byte_length).map_or(true, |x| x > max)
            || self.sha256.len() != 64
            || !self.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ApiError::invalid("Invalid original name, type, length, operation ID or SHA-256"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Photo {
    pub id: Uuid,
    pub library_id: Uuid,
    pub object_id: Uuid,
    pub operation_id: Uuid,
    pub original_name: String,
    pub media_type: String,
    pub byte_length: i64,
    pub sha256: String,
    pub metadata: serde_json::Value,
    pub status: String,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub stored_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
}
impl Photo {
    pub fn matches(&self, input: &NewPhoto) -> bool {
        self.original_name == input.original_name
            && self.media_type == input.media_type
            && self.byte_length == input.byte_length
            && self.sha256 == input.sha256
            && serde_json::to_value(&input.metadata).is_ok_and(|m| m == self.metadata)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub after: Option<Uuid>,
    pub limit: Option<i64>,
}
impl Page {
    pub fn limit(&self) -> Result<i64> {
        let n = self.limit.unwrap_or(50);
        if !(1..=100).contains(&n) { Err(ApiError::invalid("limit must be between 1 and 100")) } else { Ok(n) }
    }
}
