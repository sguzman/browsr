use std::net::SocketAddr;
use std::time::Duration;

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

impl AppConfig {
    pub fn from_env() -> Self {
        let mut config = Self::default();

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

        config
    }

    pub fn socket_addr(&self) -> SocketAddr {
        format!("{}:{}", self.bind_host, self.port)
            .parse()
            .expect("invalid bind host/port")
    }
}
