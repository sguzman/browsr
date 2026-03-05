use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub bind_host: String,
    pub port: u16,
    pub ws_path: String,
    pub request_timeout: Duration,
    pub max_incoming_ws_bytes: usize,
    pub recent_events_limit: usize,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            bind_host: "127.0.0.1".to_string(),
            port: 17373,
            ws_path: "/ws".to_string(),
            request_timeout: Duration::from_secs(8),
            max_incoming_ws_bytes: 20_000_000,
            recent_events_limit: 500,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct FileConfig {
    bind_host: Option<String>,
    port: Option<u16>,
    ws_path: Option<String>,
    request_timeout_ms: Option<u64>,
    max_incoming_ws_bytes: Option<usize>,
    recent_events_limit: Option<usize>,
}

impl AppConfig {
    pub fn load() -> Result<Self, String> {
        let mut config = Self::default();
        let config_path =
            std::env::var("BROWSR_CONFIG").unwrap_or_else(|_| "config/server.toml".to_string());

        if Path::new(&config_path).exists() {
            let raw = std::fs::read_to_string(&config_path)
                .map_err(|error| format!("failed reading config file `{config_path}`: {error}"))?;
            let parsed: FileConfig = toml::from_str(&raw)
                .map_err(|error| format!("failed parsing config file `{config_path}`: {error}"))?;
            config.apply_file(parsed);
            info!(path = %config_path, "loaded server config file");
        } else {
            warn!(path = %config_path, "config file not found, using defaults/env overrides");
        }

        if let Ok(value) = std::env::var("BROWSR_HOST") {
            config.bind_host = value;
        }
        if let Ok(value) = std::env::var("BROWSR_PORT") {
            if let Ok(parsed) = value.parse::<u16>() {
                config.port = parsed;
            }
        }
        if let Ok(value) = std::env::var("BROWSR_WS_PATH") {
            config.ws_path = value;
        }
        if let Ok(value) = std::env::var("BROWSR_REQUEST_TIMEOUT_MS") {
            if let Ok(parsed) = value.parse::<u64>() {
                config.request_timeout = Duration::from_millis(parsed);
            }
        }
        if let Ok(value) = std::env::var("BROWSR_MAX_WS_BYTES") {
            if let Ok(parsed) = value.parse::<usize>() {
                config.max_incoming_ws_bytes = parsed;
            }
        }
        if let Ok(value) = std::env::var("BROWSR_EVENTS_LIMIT") {
            if let Ok(parsed) = value.parse::<usize>() {
                config.recent_events_limit = parsed;
            }
        }

        Ok(config)
    }

    pub fn socket_addr(&self) -> SocketAddr {
        format!("{}:{}", self.bind_host, self.port)
            .parse()
            .expect("invalid bind host/port")
    }

    fn apply_file(&mut self, file: FileConfig) {
        if let Some(value) = file.bind_host {
            self.bind_host = value;
        }
        if let Some(value) = file.port {
            self.port = value;
        }
        if let Some(value) = file.ws_path {
            self.ws_path = value;
        }
        if let Some(value) = file.request_timeout_ms {
            self.request_timeout = Duration::from_millis(value);
        }
        if let Some(value) = file.max_incoming_ws_bytes {
            self.max_incoming_ws_bytes = value;
        }
        if let Some(value) = file.recent_events_limit {
            self.recent_events_limit = value;
        }
    }
}
