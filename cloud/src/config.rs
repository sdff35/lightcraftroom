use crate::error::{ApiError, Result};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{env, path::PathBuf, time::Duration};

#[derive(Clone)]
pub struct Config {
    pub listen: std::net::SocketAddr,
    pub storage: PathBuf,
    pub issuer: String,
    pub audience: String,
    pub jwks_url: String,
    pub allow_http_oidc: bool,
    pub allow_http_jwks: bool,
    pub max_upload: u64,
    pub quota: i64,
    pub min_free: u64,
    pub upload_timeout: Duration,
    pub db_host: String,
    pub db_port: u16,
    pub db_name: String,
    pub db_user: String,
    pub db_password_file: PathBuf,
}
fn value(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.into())
}
fn number<T: std::str::FromStr>(name: &str, default: &str) -> Result<T> {
    value(name, default).parse().map_err(|_| ApiError::invalid("Invalid numeric configuration"))
}
impl Config {
    pub fn from_env() -> Result<Self> {
        let c = Self {
            listen: value("LC_LISTEN", "0.0.0.0:8080").parse().map_err(|_| ApiError::invalid("Invalid listen address"))?,
            storage: value("LC_STORAGE_ROOT", "/data").into(),
            issuer: env::var("LC_OIDC_ISSUER").map_err(|_| ApiError::invalid("LC_OIDC_ISSUER is required"))?,
            audience: value("LC_OIDC_AUDIENCE", "lightcraft-cloud"),
            jwks_url: env::var("LC_OIDC_JWKS_URL").map_err(|_| ApiError::invalid("LC_OIDC_JWKS_URL is required"))?,
            allow_http_oidc: value("LC_ALLOW_HTTP_OIDC", "false") == "true",
            allow_http_jwks: value("LC_ALLOW_HTTP_JWKS", "false") == "true",
            max_upload: number("LC_MAX_UPLOAD_BYTES", "536870912")?,
            quota: number("LC_STORAGE_QUOTA_BYTES", "10737418240")?,
            min_free: number("LC_MIN_FREE_BYTES", "21474836480")?,
            upload_timeout: Duration::from_secs(number("LC_UPLOAD_TIMEOUT_SECONDS", "300")?),
            db_host: value("LC_DB_HOST", "lc-db"),
            db_port: number("LC_DB_PORT", "5432")?,
            db_name: value("LC_DB_NAME", "lightcraft"),
            db_user: value("LC_DB_USER", "lightcraft_api"),
            db_password_file: value("LC_DB_PASSWORD_FILE", "/run/secrets/pg_api_password").into(),
        };
        c.validate()?;
        Ok(c)
    }
    pub fn validate(&self) -> Result<()> {
        if self.max_upload < 16
            || self.max_upload > 536_870_912
            || self.quota < 16
            || self.quota > i64::MAX - 536_870_912
            || self.upload_timeout.is_zero()
            || self.upload_timeout > Duration::from_secs(900)
        {
            return Err(ApiError::invalid("Invalid resource limits"));
        }
        for (index, value) in [&self.issuer, &self.jwks_url].into_iter().enumerate() {
            let u = reqwest::Url::parse(value).map_err(|_| ApiError::invalid("Invalid OIDC URL"))?;
            if u.host_str().is_none()
                || !u.username().is_empty()
                || u.password().is_some()
                || u.fragment().is_some()
                || u.query().is_some()
                || (u.scheme() != "https"
                    && !(u.scheme() == "http"
                        && (self.allow_http_oidc
                            || (index == 1 && self.allow_http_jwks && u.host_str() == Some("lc-auth") && u.port() == Some(8080)))))
            {
                return Err(ApiError::invalid("OIDC requires HTTPS; HTTP must be explicitly enabled for isolated local development"));
            }
        }
        if self.issuer.ends_with('/') || self.audience.is_empty() {
            return Err(ApiError::invalid("Invalid OIDC issuer or audience"));
        }
        Ok(())
    }
    pub async fn pool(&self) -> Result<sqlx::PgPool> {
        let password =
            tokio::fs::read_to_string(&self.db_password_file).await.map_err(|_| ApiError::invalid("Database password file is unavailable"))?;
        if password.trim().is_empty() {
            return Err(ApiError::invalid("Database password is empty"));
        }
        let options = PgConnectOptions::new()
            .host(&self.db_host)
            .port(self.db_port)
            .database(&self.db_name)
            .username(&self.db_user)
            .password(password.trim())
            .application_name("lightcraft-cloud");
        Ok(PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(5))
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET statement_timeout = '15s'").execute(&mut *conn).await?;
                    sqlx::query("SET idle_in_transaction_session_timeout = '16min'").execute(conn).await?;
                    Ok(())
                })
            })
            .connect_with(options)
            .await?)
    }
}
