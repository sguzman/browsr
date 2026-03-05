use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde_json::Value;
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::config::AppConfig;
use crate::protocol::CommandEnvelope;

#[derive(Debug)]
pub enum SendCommandError {
    ExtensionNotConnected,
    TransportClosed,
    Timeout,
    ResponseDropped,
    InvalidMessage(String),
}

#[derive(Debug, Clone)]
pub struct ExtensionSnapshot {
    pub connected: bool,
    pub connected_since: Option<DateTime<Utc>>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub last_hello: Option<Value>,
    pub last_tabs: Option<Value>,
    pub last_windows: Option<Value>,
    pub pending_requests: usize,
    pub recent_events: Vec<Value>,
}

#[derive(Debug, Clone)]
struct ExtensionConnection {
    sender: mpsc::Sender<String>,
}

pub struct AppState {
    config: AppConfig,
    ext_connection: RwLock<Option<ExtensionConnection>>,
    connected_since: RwLock<Option<DateTime<Utc>>>,
    last_message_at: RwLock<Option<DateTime<Utc>>>,
    last_hello: RwLock<Option<Value>>,
    last_tabs: RwLock<Option<Value>>,
    last_windows: RwLock<Option<Value>>,
    recent_events: Mutex<VecDeque<Value>>,
    pending: DashMap<String, oneshot::Sender<Value>>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        let recent_events_limit = config.recent_events_limit;
        Self {
            config,
            ext_connection: RwLock::new(None),
            connected_since: RwLock::new(None),
            last_message_at: RwLock::new(None),
            last_hello: RwLock::new(None),
            last_tabs: RwLock::new(None),
            last_windows: RwLock::new(None),
            recent_events: Mutex::new(VecDeque::with_capacity(recent_events_limit)),
            pending: DashMap::new(),
        }
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub async fn set_extension_sender(&self, sender: mpsc::Sender<String>) {
        let mut guard = self.ext_connection.write().await;
        *guard = Some(ExtensionConnection { sender });
        *self.connected_since.write().await = Some(Utc::now());
        info!("extension connected");
    }

    pub async fn clear_extension_sender(&self) {
        let mut guard = self.ext_connection.write().await;
        let had_connection = guard.take().is_some();
        if had_connection {
            warn!("extension disconnected; failing pending requests");
            self.fail_all_pending("EXTENSION_DISCONNECTED", "extension session disconnected");
        }
    }

    pub async fn extension_connected(&self) -> bool {
        self.ext_connection.read().await.is_some()
    }

    pub async fn record_message(&self) {
        *self.last_message_at.write().await = Some(Utc::now());
    }

    pub async fn cache_hello(&self, message: Value) {
        *self.last_hello.write().await = Some(message);
    }

    pub async fn cache_tabs(&self, tabs: Value) {
        *self.last_tabs.write().await = Some(tabs);
    }

    pub async fn cache_windows(&self, windows: Value) {
        *self.last_windows.write().await = Some(windows);
    }

    pub async fn get_tabs_cache(&self) -> Option<Value> {
        self.last_tabs.read().await.clone()
    }

    pub async fn push_event(&self, event: Value) {
        let mut guard = self.recent_events.lock().await;
        guard.push_back(event);
        while guard.len() > self.config.recent_events_limit {
            guard.pop_front();
        }
    }

    pub async fn snapshot(&self) -> ExtensionSnapshot {
        let connected = self.extension_connected().await;
        let connected_since = *self.connected_since.read().await;
        let last_message_at = *self.last_message_at.read().await;
        let last_hello = self.last_hello.read().await.clone();
        let last_tabs = self.last_tabs.read().await.clone();
        let last_windows = self.last_windows.read().await.clone();
        let recent_events = self.recent_events.lock().await.iter().cloned().collect();
        let pending_requests = self.pending.len();
        ExtensionSnapshot {
            connected,
            connected_since,
            last_message_at,
            last_hello,
            last_tabs,
            last_windows,
            pending_requests,
            recent_events,
        }
    }

    pub fn resolve_pending(&self, id: &str, payload: Value) {
        if let Some((_, sender)) = self.pending.remove(id) {
            if sender.send(payload).is_err() {
                warn!(request_id = %id, "pending receiver dropped before response");
            }
        } else {
            warn!(request_id = %id, "received response for unknown request id");
        }
    }

    pub async fn send_command(
        self: &Arc<Self>,
        command: &str,
        args: Value,
    ) -> Result<Value, SendCommandError> {
        let envelope = CommandEnvelope::new(command, args);
        let request_id = envelope.id.clone();
        let trace_id = envelope.trace_id.clone();
        let raw = serde_json::to_string(&envelope)
            .map_err(|error| SendCommandError::InvalidMessage(error.to_string()))?;

        let ext_sender = self
            .ext_connection
            .read()
            .await
            .as_ref()
            .map(|conn| conn.sender.clone())
            .ok_or(SendCommandError::ExtensionNotConnected)?;

        let (reply_tx, reply_rx) = oneshot::channel();
        self.pending.insert(request_id.clone(), reply_tx);

        let started = Instant::now();
        debug!(
            request_id = %request_id,
            trace_id = %trace_id,
            command = %command,
            bytes = raw.len(),
            "sending command to extension"
        );

        if ext_sender.send(raw).await.is_err() {
            self.pending.remove(&request_id);
            return Err(SendCommandError::TransportClosed);
        }

        let result = timeout(self.config.request_timeout, reply_rx).await;
        match result {
            Ok(Ok(message)) => {
                debug!(
                    request_id = %request_id,
                    trace_id = %trace_id,
                    command = %command,
                    elapsed_ms = started.elapsed().as_millis(),
                    "command response received"
                );
                Ok(message)
            }
            Ok(Err(_)) => {
                self.pending.remove(&request_id);
                Err(SendCommandError::ResponseDropped)
            }
            Err(_) => {
                self.pending.remove(&request_id);
                warn!(
                    request_id = %request_id,
                    trace_id = %trace_id,
                    command = %command,
                    timeout_ms = self.config.request_timeout.as_millis(),
                    "command timed out"
                );
                Err(SendCommandError::Timeout)
            }
        }
    }

    fn fail_all_pending(&self, code: &str, message: &str) {
        let ids: Vec<String> = self
            .pending
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        for id in ids {
            if let Some((_, sender)) = self.pending.remove(&id) {
                let payload = serde_json::json!({
                    "type": "response",
                    "id": id,
                    "ok": false,
                    "error": {
                        "code": code,
                        "message": message
                    }
                });
                let _ = sender.send(payload);
            }
        }
    }
}
