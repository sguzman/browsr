mod api;
mod config;
mod protocol;
mod state;
mod ws_ext;

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use chrono::Utc;
use config::AppConfig;
use state::AppState;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let _log_guard = match init_tracing(&cli) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("failed to initialize tracing: {error}");
            return;
        }
    };
    let config = match AppConfig::load() {
        Ok(config) => config,
        Err(error) => {
            error!(error = %error, "failed to load server config");
            return;
        }
    };
    let state = Arc::new(AppState::new(config.clone()));
    let addr: SocketAddr = config.socket_addr();

    let app = Router::new()
        .route(&config.ws_path, get(ws_ext::ws_handler))
        .merge(api::router())
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state);

    info!(bind = %addr, ws_path = %config.ws_path, "starting browsr server");

    match TcpListener::bind(addr).await {
        Ok(listener) => {
            if let Err(error) = axum::serve(listener, app).await {
                error!(error = %error, "server stopped with error");
            }
        }
        Err(error) => {
            error!(error = %error, bind = %addr, "failed to bind listener");
        }
    }
}

#[derive(Debug, Default)]
struct Cli {
    log_to_file: bool,
}

impl Cli {
    fn parse() -> Self {
        let log_to_file = env::args().skip(1).any(|arg| arg == "--log");
        Self { log_to_file }
    }
}

fn init_tracing(cli: &Cli) -> Result<Option<WorkerGuard>, String> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("browsr=debug,tower_http=info,axum::rejection=trace"));

    let stdout_layer = tracing_subscriber::fmt::layer().with_target(true).compact();

    if cli.log_to_file {
        let log_path = prepare_log_path()?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|error| {
                format!("failed to open log file `{}`: {error}", log_path.display())
            })?;
        let (writer, guard) = tracing_appender::non_blocking(file);
        let file_layer = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(writer)
            .with_target(true)
            .compact();

        tracing_subscriber::registry()
            .with(env_filter)
            .with(stdout_layer)
            .with(file_layer)
            .init();

        info!(path = %log_path.display(), "file logging enabled");
        Ok(Some(guard))
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(stdout_layer)
            .init();
        Ok(None)
    }
}

fn prepare_log_path() -> Result<PathBuf, String> {
    let dir = PathBuf::from("logs");
    std::fs::create_dir_all(&dir).map_err(|error| {
        format!(
            "failed to create log directory `{}`: {error}",
            dir.display()
        )
    })?;
    let filename = format!("browsr-{}.log", Utc::now().format("%Y%m%d-%H%M%S"));
    Ok(dir.join(filename))
}
