mod api;
mod config;
mod protocol;
mod state;
mod ws_ext;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use config::AppConfig;
use state::AppState;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    init_tracing();
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

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("browsr=debug,tower_http=info,axum::rejection=trace"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(true)
        .compact()
        .init();
}
