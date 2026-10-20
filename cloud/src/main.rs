#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
use lightcraft_cloud::{AppState, api, auth::Oidc, config::Config, error::ApiError, storage::Storage};
use std::sync::Arc;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "lightcraft_cloud=info".into()))
        .with_target(false)
        .init();
    let provider = rustls_rustcrypto::provider();
    if provider.install_default().is_err() {
        tracing::error!("TLS crypto provider initialization failed");
        return std::process::ExitCode::FAILURE;
    }
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(code = e.code, message = e.message, "LightCraft Cloud stopped");
            std::process::ExitCode::FAILURE
        }
    }
}
async fn run() -> Result<(), ApiError> {
    if std::env::args().any(|a| a == "--version") {
        println!("lightcraft-cloud {} {}", env!("CARGO_PKG_VERSION"), std::env::consts::ARCH);
        return Ok(());
    }
    let config = Config::from_env()?;
    if std::env::args().any(|a| a == "--healthcheck") {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let address = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), config.listen.port());
        let mut stream = tokio::time::timeout(std::time::Duration::from_secs(3), tokio::net::TcpStream::connect(address))
            .await
            .map_err(|_| ApiError::unavailable())?
            .map_err(|_| ApiError::unavailable())?;
        stream.write_all(b"GET /health/ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await?;
        let mut buf = [0u8; 64];
        let size = tokio::time::timeout(std::time::Duration::from_secs(8), stream.read(&mut buf)).await.map_err(|_| ApiError::unavailable())??;
        if !buf.get(..size).is_some_and(|b| b.starts_with(b"HTTP/1.1 200")) {
            return Err(ApiError::unavailable());
        }
        return Ok(());
    }
    let pool = config.pool().await?;
    if std::env::args().any(|a| a == "--migrate") {
        sqlx::migrate!("./migrations").run(&pool).await.map_err(|_| ApiError::unavailable())?;
        tracing::info!("PostgreSQL migrations applied");
        return Ok(());
    }
    let storage = Storage::open(&config.storage)?;
    lightcraft_cloud::reconcile(&pool, &storage).await?;
    let oidc = Oidc::new(config.issuer.clone(), config.audience.clone(), config.jwks_url.clone())?;
    oidc.ready().await?;
    let address = config.listen;
    let state = AppState { config: Arc::new(config), pool, oidc, storage, file_ops: Arc::new(tokio::sync::Semaphore::new(2)) };
    let listener = tokio::net::TcpListener::bind(address).await.map_err(|_| ApiError::unavailable())?;
    tracing::info!(%address,"LightCraft Cloud listening");
    axum::serve(listener, api::router(state)).with_graceful_shutdown(shutdown()).await.map_err(|_| ApiError::unavailable())?;
    Ok(())
}
async fn shutdown() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {_ = tokio::signal::ctrl_c()=>{},_ = terminate.recv()=>{}}
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
