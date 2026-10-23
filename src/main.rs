use anyhow::{Context, Result};
use farm_service::{api, assets, config::Config, db};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::from_env()?;
    std::fs::create_dir_all(&config.log_dir)?;
    let file = tracing_appender::rolling::daily(&config.log_dir, "farm_service.jsonl");
    let (writer, _log_guard) = tracing_appender::non_blocking(file);
    tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("farm_service=info")),
        )
        .with_writer(writer)
        .init();

    let bundle = config
        .asset_manifest
        .as_deref()
        .map(assets::AssetBundle::load)
        .transpose()
        .context("asset startup verification failed")?;
    std::fs::create_dir_all(&config.data_dir)?;
    let pool = db::open_database(&config.data_dir.join("farm_service.sqlite3"))
        .await
        .context("database startup failed")?;
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    tracing::info!(address = %listener.local_addr()?, "service_ready");
    let app = api::router(pool.clone(), config.api_token).merge(assets::router(bundle));
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await;
    pool.close().await;
    result.context("HTTP server failed")?;
    tracing::info!("service_stopped");
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate =
            signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
