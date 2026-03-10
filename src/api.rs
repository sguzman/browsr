use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::sleep;
use tracing::{debug, info};

use crate::state::{AppState, SendCommandError};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/v1/status", get(status))
        .route("/v1/windows", get(list_windows))
        .route("/v1/tabs/active", get(get_active_tab))
        .route("/v1/tabs/open", post(open_tab))
        .route("/v1/tabs/refresh", post(refresh_tabs))
        .route("/v1/tabs", get(list_tabs))
        .route("/v1/tabs/{tab_id}", get(get_tab_state))
        .route("/v1/tabs/{tab_id}/snapshot", post(snapshot_tab))
        .route("/v1/tabs/{tab_id}/focus", post(focus_tab))
        .route(
            "/v1/tabs/{tab_id}/import-bundles",
            post(start_import_bundle),
        )
        .route(
            "/v1/tabs/{tab_id}/import-bundles/wait",
            post(start_import_bundle_and_wait),
        )
        .route("/v1/tabs/{tab_id}/reload", post(reload_tab))
        .route("/v1/tabs/{tab_id}/close", post(close_tab))
        .route("/v1/tabs/{tab_id}/move", post(move_tab))
        .route("/v1/tab-groups", post(group_tabs))
        .route("/v1/import-bundles/{job_id}", get(get_import_bundle_status))
        .route(
            "/v1/import-bundles/{job_id}/wait",
            get(wait_for_import_bundle),
        )
        .route(
            "/v1/import-bundles/{job_id}/manifest",
            get(get_import_bundle_manifest),
        )
        .route(
            "/v1/import-bundles/{job_id}/assets/{asset_id}",
            get(get_import_bundle_asset),
        )
        .route(
            "/v1/import-bundles/{job_id}/cancel",
            post(cancel_import_bundle),
        )
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
struct ActiveTabQuery {
    window_id: Option<u32>,
}

async fn get_active_tab(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ActiveTabQuery>,
) -> Result<Json<Value>, ApiError> {
    let args = match query.window_id {
        Some(window_id) => json!({ "windowId": window_id }),
        None => json!({}),
    };
    let response = send_command_checked(&state, "get_active_tab", args).await?;
    if let Some(tab) = response.get("tab") {
        state.upsert_tab_cache_entry(tab).await;
    }
    Ok(Json(response))
}

async fn get_tab_state(
    State(state): State<Arc<AppState>>,
    Path(tab_id): Path<u32>,
) -> Result<Json<Value>, ApiError> {
    let response =
        send_command_checked(&state, "get_tab_state", json!({ "tabId": tab_id })).await?;
    if let Some(tab) = response.get("tab") {
        state.upsert_tab_cache_entry(tab).await;
    }
    Ok(Json(response))
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

#[derive(Debug, Deserialize)]
struct ImportBundleStartRequest {
    reload: Option<bool>,
    capture_html: Option<bool>,
    capture_assets: Option<bool>,
    capture_text: Option<bool>,
    capture_selection: Option<bool>,
    capture_screenshot: Option<bool>,
    wait_for_network_idle_ms: Option<u64>,
    settle_timeout_ms: Option<u64>,
    max_asset_bytes: Option<u64>,
    max_total_bytes: Option<u64>,
    wait_timeout_ms: Option<u64>,
    poll_interval_ms: Option<u64>,
    include_manifest: Option<bool>,
}

async fn start_import_bundle(
    State(state): State<Arc<AppState>>,
    Path(tab_id): Path<u32>,
    Json(body): Json<ImportBundleStartRequest>,
) -> Result<Json<Value>, ApiError> {
    let mut args = json!({ "tabId": tab_id });
    if let Some(reload) = body.reload {
        args["reload"] = json!(reload);
    }
    if let Some(capture_html) = body.capture_html {
        args["captureHtml"] = json!(capture_html);
    }
    if let Some(capture_assets) = body.capture_assets {
        args["captureAssets"] = json!(capture_assets);
    }
    if let Some(capture_text) = body.capture_text {
        args["captureText"] = json!(capture_text);
    }
    if let Some(capture_selection) = body.capture_selection {
        args["captureSelection"] = json!(capture_selection);
    }
    if let Some(capture_screenshot) = body.capture_screenshot {
        args["captureScreenshot"] = json!(capture_screenshot);
    }
    if let Some(wait_for_network_idle_ms) = body.wait_for_network_idle_ms {
        args["waitForNetworkIdleMs"] = json!(wait_for_network_idle_ms);
    }
    if let Some(settle_timeout_ms) = body.settle_timeout_ms {
        args["settleTimeoutMs"] = json!(settle_timeout_ms);
    }
    if let Some(max_asset_bytes) = body.max_asset_bytes {
        args["maxAssetBytes"] = json!(max_asset_bytes);
    }
    if let Some(max_total_bytes) = body.max_total_bytes {
        args["maxTotalBytes"] = json!(max_total_bytes);
    }

    let response = send_command_checked(&state, "start_import_bundle", args).await?;
    Ok(Json(response))
}

async fn start_import_bundle_and_wait(
    State(state): State<Arc<AppState>>,
    Path(tab_id): Path<u32>,
    Json(body): Json<ImportBundleStartRequest>,
) -> Result<Json<Value>, ApiError> {
    let wait_timeout_ms = body.wait_timeout_ms;
    let poll_interval_ms = body.poll_interval_ms;
    let include_manifest = body.include_manifest.unwrap_or(true);

    let args = build_import_bundle_args(tab_id, &body);
    let started = send_command_checked(&state, "start_import_bundle", args).await?;
    let job_id = started
        .get("jobId")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_gateway("start_import_bundle response missing jobId"))?
        .to_string();

    let wait = wait_for_import_bundle_inner(
        &state,
        &job_id,
        wait_timeout_ms,
        poll_interval_ms,
        include_manifest,
    )
    .await?;

    Ok(Json(json!({
        "started": started,
        "result": wait
    })))
}

#[derive(Debug, Deserialize)]
struct OpenTabRequest {
    url: String,
    active: Option<bool>,
    window_id: Option<u32>,
    index: Option<u32>,
    opener_tab_id: Option<u32>,
}

async fn open_tab(
    State(state): State<Arc<AppState>>,
    Json(body): Json<OpenTabRequest>,
) -> Result<Json<Value>, ApiError> {
    let mut args = json!({ "url": body.url });
    if let Some(active) = body.active {
        args["active"] = json!(active);
    }
    if let Some(window_id) = body.window_id {
        args["windowId"] = json!(window_id);
    }
    if let Some(index) = body.index {
        args["index"] = json!(index);
    }
    if let Some(opener_tab_id) = body.opener_tab_id {
        args["openerTabId"] = json!(opener_tab_id);
    }

    let response = send_command_checked(&state, "open_tab", args).await?;
    if let Some(tab) = response.get("tab") {
        state.upsert_tab_cache_entry(tab).await;
    }
    Ok(Json(response))
}

async fn focus_tab(
    State(state): State<Arc<AppState>>,
    Path(tab_id): Path<u32>,
) -> Result<Json<Value>, ApiError> {
    let response = send_command_checked(&state, "focus_tab", json!({ "tabId": tab_id })).await?;
    if let Some(tab) = response.get("tab") {
        state.upsert_tab_cache_entry(tab).await;
    }
    if let Some(window) = response.get("window") {
        state.upsert_window_cache_entry(window).await;
    }
    Ok(Json(response))
}

#[derive(Debug, Deserialize)]
struct ReloadTabRequest {
    #[serde(default)]
    bypass_cache: bool,
    #[serde(default)]
    wait_for_complete: bool,
}

async fn reload_tab(
    State(state): State<Arc<AppState>>,
    Path(tab_id): Path<u32>,
    Json(body): Json<ReloadTabRequest>,
) -> Result<Json<Value>, ApiError> {
    let response = send_command_checked(
        &state,
        "reload_tab",
        json!({
            "tabId": tab_id,
            "bypassCache": body.bypass_cache,
            "waitForComplete": body.wait_for_complete
        }),
    )
    .await?;
    if let Some(tab) = response.get("tab") {
        state.upsert_tab_cache_entry(tab).await;
    }
    Ok(Json(response))
}

async fn close_tab(
    State(state): State<Arc<AppState>>,
    Path(tab_id): Path<u32>,
) -> Result<Json<Value>, ApiError> {
    let response = send_command_checked(&state, "close_tab", json!({ "tabId": tab_id })).await?;
    state.remove_tab_cache_entry(tab_id as u64).await;
    Ok(Json(response))
}

#[derive(Debug, Deserialize)]
struct MoveTabRequest {
    index: u32,
    window_id: Option<u32>,
}

async fn move_tab(
    State(state): State<Arc<AppState>>,
    Path(tab_id): Path<u32>,
    Json(body): Json<MoveTabRequest>,
) -> Result<Json<Value>, ApiError> {
    let mut args = json!({
        "tabId": tab_id,
        "index": body.index
    });
    if let Some(window_id) = body.window_id {
        args["windowId"] = json!(window_id);
    }

    let response = send_command_checked(&state, "move_tab", args).await?;
    if let Some(tab) = response.get("tab") {
        state.upsert_tab_cache_entry(tab).await;
    }
    Ok(Json(response))
}

#[derive(Debug, Deserialize)]
struct CreatePropertiesRequest {
    window_id: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct GroupPropertiesRequest {
    title: Option<String>,
    color: Option<String>,
    collapsed: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct GroupTabsRequest {
    tab_ids: Vec<u32>,
    group_id: Option<u32>,
    create_properties: Option<CreatePropertiesRequest>,
    group_properties: Option<GroupPropertiesRequest>,
}

async fn group_tabs(
    State(state): State<Arc<AppState>>,
    Json(body): Json<GroupTabsRequest>,
) -> Result<Json<Value>, ApiError> {
    let mut args = json!({
        "tabIds": body.tab_ids
    });
    if let Some(group_id) = body.group_id {
        args["groupId"] = json!(group_id);
    }
    if let Some(create_properties) = body.create_properties {
        let mut create = json!({});
        if let Some(window_id) = create_properties.window_id {
            create["windowId"] = json!(window_id);
        }
        args["createProperties"] = create;
    }
    if let Some(group_properties) = body.group_properties {
        let mut group = json!({});
        if let Some(title) = group_properties.title {
            group["title"] = json!(title);
        }
        if let Some(color) = group_properties.color {
            group["color"] = json!(color);
        }
        if let Some(collapsed) = group_properties.collapsed {
            group["collapsed"] = json!(collapsed);
        }
        args["groupProperties"] = group;
    }

    let response = send_command_checked(&state, "group_tabs", args).await?;
    if let Some(tabs) = response.get("tabs").and_then(Value::as_array) {
        state.upsert_tab_cache_entries(tabs.iter()).await;
    }
    Ok(Json(response))
}

async fn get_import_bundle_status(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let response = send_command_checked(
        &state,
        "get_import_bundle_status",
        json!({ "jobId": job_id }),
    )
    .await?;
    Ok(Json(response))
}

async fn get_import_bundle_manifest(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let response = send_command_checked(
        &state,
        "get_import_bundle_manifest",
        json!({ "jobId": job_id }),
    )
    .await?;
    Ok(Json(response))
}

#[derive(Debug, Deserialize)]
struct ImportBundleAssetQuery {
    offset: Option<u64>,
    length: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ImportBundleWaitQuery {
    timeout_ms: Option<u64>,
    poll_interval_ms: Option<u64>,
    include_manifest: Option<bool>,
}

async fn get_import_bundle_asset(
    State(state): State<Arc<AppState>>,
    Path((job_id, asset_id)): Path<(String, String)>,
    Query(query): Query<ImportBundleAssetQuery>,
) -> Result<Json<Value>, ApiError> {
    let mut args = json!({
        "jobId": job_id,
        "assetId": asset_id
    });
    if let Some(offset) = query.offset {
        args["offset"] = json!(offset);
    }
    if let Some(length) = query.length {
        args["length"] = json!(length);
    }

    let response = send_command_checked(&state, "get_import_bundle_asset", args).await?;
    Ok(Json(response))
}

async fn wait_for_import_bundle(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
    Query(query): Query<ImportBundleWaitQuery>,
) -> Result<Json<Value>, ApiError> {
    let response = wait_for_import_bundle_inner(
        &state,
        &job_id,
        query.timeout_ms,
        query.poll_interval_ms,
        query.include_manifest.unwrap_or(true),
    )
    .await?;
    Ok(Json(response))
}

async fn cancel_import_bundle(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let response =
        send_command_checked(&state, "cancel_import_bundle", json!({ "jobId": job_id })).await?;
    Ok(Json(response))
}

fn build_import_bundle_args(tab_id: u32, body: &ImportBundleStartRequest) -> Value {
    let mut args = json!({ "tabId": tab_id });
    if let Some(reload) = body.reload {
        args["reload"] = json!(reload);
    }
    if let Some(capture_html) = body.capture_html {
        args["captureHtml"] = json!(capture_html);
    }
    if let Some(capture_assets) = body.capture_assets {
        args["captureAssets"] = json!(capture_assets);
    }
    if let Some(capture_text) = body.capture_text {
        args["captureText"] = json!(capture_text);
    }
    if let Some(capture_selection) = body.capture_selection {
        args["captureSelection"] = json!(capture_selection);
    }
    if let Some(capture_screenshot) = body.capture_screenshot {
        args["captureScreenshot"] = json!(capture_screenshot);
    }
    if let Some(wait_for_network_idle_ms) = body.wait_for_network_idle_ms {
        args["waitForNetworkIdleMs"] = json!(wait_for_network_idle_ms);
    }
    if let Some(settle_timeout_ms) = body.settle_timeout_ms {
        args["settleTimeoutMs"] = json!(settle_timeout_ms);
    }
    if let Some(max_asset_bytes) = body.max_asset_bytes {
        args["maxAssetBytes"] = json!(max_asset_bytes);
    }
    if let Some(max_total_bytes) = body.max_total_bytes {
        args["maxTotalBytes"] = json!(max_total_bytes);
    }
    args
}

async fn wait_for_import_bundle_inner(
    state: &Arc<AppState>,
    job_id: &str,
    timeout_ms: Option<u64>,
    poll_interval_ms: Option<u64>,
    include_manifest: bool,
) -> Result<Value, ApiError> {
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(120_000).max(1_000));
    let poll_interval = Duration::from_millis(poll_interval_ms.unwrap_or(500).max(100));
    let deadline = Instant::now() + timeout;

    loop {
        let status = send_command_checked(
            state,
            "get_import_bundle_status",
            json!({ "jobId": job_id }),
        )
        .await?;

        let current_status = status
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::bad_gateway("import bundle status response missing status"))?;

        match current_status {
            "completed" => {
                if include_manifest {
                    let manifest = send_command_checked(
                        state,
                        "get_import_bundle_manifest",
                        json!({ "jobId": job_id }),
                    )
                    .await?;
                    return Ok(json!({
                        "job": status,
                        "manifest": manifest
                    }));
                }
                return Ok(json!({ "job": status }));
            }
            "failed" | "cancelled" => {
                return Ok(json!({ "job": status }));
            }
            _ => {}
        }

        if Instant::now() >= deadline {
            return Err(ApiError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "IMPORT_BUNDLE_WAIT_TIMEOUT",
                format!("timed out waiting for import bundle job {job_id}"),
            ));
        }

        sleep(poll_interval).await;
    }
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
