use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{debug, info};

use crate::state::{AppState, SendCommandError};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/v1/status", get(status))
        .route("/v1/windows", get(list_windows))
        .route("/v1/tabs", get(list_tabs))
        .route("/v1/tabs/refresh", post(refresh_tabs))
        .route("/v1/tabs/{tab_id}/snapshot", post(snapshot_tab))
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    ok: bool,
    extension_connected: bool,
    now: DateTime<Utc>,
}

async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        extension_connected: state.extension_connected().await,
        now: Utc::now(),
    })
}

#[derive(Debug, Serialize)]
struct StatusResponse {
    extension_connected: bool,
    connected_since: Option<DateTime<Utc>>,
    last_message_at: Option<DateTime<Utc>>,
    pending_requests: usize,
    recent_events: usize,
    last_hello: Option<Value>,
    last_windows: Option<Value>,
    last_tabs: Option<Value>,
}

async fn status(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let snapshot = state.snapshot().await;
    Json(StatusResponse {
        extension_connected: snapshot.connected,
        connected_since: snapshot.connected_since,
        last_message_at: snapshot.last_message_at,
        pending_requests: snapshot.pending_requests,
        recent_events: snapshot.recent_events.len(),
        last_hello: snapshot.last_hello,
        last_windows: snapshot.last_windows,
        last_tabs: snapshot.last_tabs,
    })
}

async fn list_windows(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let response = send_command_checked(&state, "list_windows", json!({})).await?;
    if let Some(windows) = response.get("windows") {
        state.cache_windows(windows.clone()).await;
    }
    Ok(Json(response))
}

#[derive(Debug, Deserialize)]
struct TabsQuery {
    window_id: Option<u32>,
    q: Option<String>,
    refresh: Option<bool>,
}

async fn list_tabs(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TabsQuery>,
) -> Result<Json<Value>, ApiError> {
    let mut tabs = if query.refresh.unwrap_or(false) {
        refresh_tabs_inner(&state, query.window_id).await?
    } else if let Some(cached) = state.get_tabs_cache().await {
        cached
    } else {
        refresh_tabs_inner(&state, query.window_id).await?
    };

    if query.window_id.is_some() || query.q.as_deref().is_some() {
        tabs = filter_tabs(tabs, query.window_id, query.q.as_deref());
    }

    Ok(Json(json!({ "tabs": tabs })))
}

async fn refresh_tabs(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let tabs = refresh_tabs_inner(&state, None).await?;
    Ok(Json(json!({ "tabs": tabs })))
}

#[derive(Debug, Deserialize)]
struct SnapshotRequest {
    #[serde(default = "true_value")]
    include_html: bool,
    #[serde(default = "true_value")]
    include_text: bool,
    #[serde(default = "true_value")]
    include_selection: bool,
}

fn true_value() -> bool {
    true
}

async fn snapshot_tab(
    State(state): State<Arc<AppState>>,
    Path(tab_id): Path<u32>,
    Json(body): Json<SnapshotRequest>,
) -> Result<Json<Value>, ApiError> {
    let args = json!({
        "tabId": tab_id,
        "includeHtml": body.include_html,
        "includeText": body.include_text,
        "includeSelection": body.include_selection
    });
    let response = send_command_checked(&state, "snapshot_tab", args).await?;
    Ok(Json(response))
}

async fn refresh_tabs_inner(
    state: &Arc<AppState>,
    window_id: Option<u32>,
) -> Result<Value, ApiError> {
    let args = match window_id {
        Some(id) => json!({ "windowId": id }),
        None => json!({}),
    };

    let response = send_command_checked(state, "list_tabs", args).await?;
    let tabs = response
        .get("tabs")
        .cloned()
        .ok_or_else(|| ApiError::bad_gateway("extension response missing tabs field"))?;
    state.cache_tabs(tabs.clone()).await;
    info!(
        tabs = tabs.as_array().map_or(0, Vec::len),
        "tabs cache refreshed"
    );
    Ok(tabs)
}

async fn send_command_checked(
    state: &Arc<AppState>,
    command: &str,
    args: Value,
) -> Result<Value, ApiError> {
    let response = state
        .send_command(command, args)
        .await
        .map_err(ApiError::from_send_error)?;

    let ok = response
        .get("ok")
        .and_then(Value::as_bool)
        .ok_or_else(|| ApiError::bad_gateway("extension response missing ok field"))?;

    if ok {
        let result = response.get("result").cloned().ok_or_else(|| {
            ApiError::bad_gateway("extension success response missing result field")
        })?;
        debug!(command = %command, "extension command succeeded");
        Ok(result)
    } else {
        let code = response
            .get("error")
            .and_then(|err| err.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("COMMAND_FAILED");
        let message = response
            .get("error")
            .and_then(|err| err.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("extension returned error without message");
        Err(ApiError::bad_gateway(format!(
            "extension command failed ({code}): {message}"
        )))
    }
}

fn filter_tabs(tabs: Value, window_id: Option<u32>, q: Option<&str>) -> Value {
    let mut list = tabs.as_array().cloned().unwrap_or_default();

    if let Some(id) = window_id {
        list.retain(|tab| {
            tab.get("windowId")
                .and_then(Value::as_u64)
                .map(|value| value == id as u64)
                .unwrap_or(false)
        });
    }

    if let Some(search) = q {
        let needle = search.to_lowercase();
        list.retain(|tab| {
            let title = tab.get("title").and_then(Value::as_str).unwrap_or("");
            let url = tab.get("url").and_then(Value::as_str).unwrap_or("");
            title.to_lowercase().contains(&needle) || url.to_lowercase().contains(&needle)
        });
    }

    Value::Array(list)
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, "EXTENSION_ERROR", message)
    }

    fn from_send_error(error: SendCommandError) -> Self {
        match error {
            SendCommandError::ExtensionNotConnected => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "EXTENSION_NOT_CONNECTED",
                "extension is not connected to /ws",
            ),
            SendCommandError::TransportClosed => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "EXTENSION_DISCONNECTED",
                "extension connection closed before command could be sent",
            ),
            SendCommandError::Timeout => Self::new(
                StatusCode::GATEWAY_TIMEOUT,
                "EXTENSION_TIMEOUT",
                "extension command timed out",
            ),
            SendCommandError::ResponseDropped => Self::new(
                StatusCode::BAD_GATEWAY,
                "EXTENSION_RESPONSE_DROPPED",
                "extension response channel dropped unexpectedly",
            ),
            SendCommandError::InvalidMessage(message) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "COMMAND_SERIALIZATION_FAILED",
                message,
            ),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let payload = Json(json!({
            "ok": false,
            "error": {
                "code": self.code,
                "message": self.message
            }
        }));
        (self.status, payload).into_response()
    }
}
