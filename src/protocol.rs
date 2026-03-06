use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    #[serde(rename = "traceId")]
    pub trace_id: String,
    pub command: String,
    pub args: Value,
}

impl CommandEnvelope {
    pub fn new(command: &str, args: Value) -> Self {
        let id = Uuid::new_v4().to_string();
        let trace_id = format!("srv-{}-{}", Utc::now().timestamp_millis(), Uuid::new_v4());
        Self {
            kind: "command".to_string(),
            id,
            trace_id,
            command: command.to_string(),
            args,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncomingKind {
    Hello,
    Event,
    Log,
    Response,
    Keepalive,
    Unknown,
}

pub fn classify_incoming(message: &Value) -> IncomingKind {
    match message.get("type").and_then(Value::as_str) {
        Some("hello") => IncomingKind::Hello,
        Some("event") => IncomingKind::Event,
        Some("log") => IncomingKind::Log,
        Some("response") => IncomingKind::Response,
        Some("keepalive") => IncomingKind::Keepalive,
        _ => IncomingKind::Unknown,
    }
}
