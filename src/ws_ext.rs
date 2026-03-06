use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::protocol::{IncomingKind, classify_incoming};
use crate::state::AppState;

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| handle_connection(state, socket))
}

async fn handle_connection(state: Arc<AppState>, socket: WebSocket) {
    let (outgoing, mut incoming) = socket.split();
    let (outbox_tx, mut outbox_rx) = mpsc::channel::<String>(256);

    state.set_extension_sender(outbox_tx).await;
    let state_for_writer = state.clone();
    let writer_handle = tokio::spawn(async move {
        let mut outgoing = outgoing;
        while let Some(payload) = outbox_rx.recv().await {
            if let Err(error) = outgoing.send(Message::Text(payload.into())).await {
                warn!(error = %error, "failed to write websocket message to extension");
                break;
            }
        }
        let _ = outgoing.send(Message::Close(None)).await;
        state_for_writer.clear_extension_sender().await;
    });

    info!("websocket connection upgraded for extension");

    while let Some(message) = incoming.next().await {
        match message {
            Ok(Message::Text(text)) => {
                if text.len() > state.config().max_incoming_ws_bytes {
                    warn!(
                        bytes = text.len(),
                        max_bytes = state.config().max_incoming_ws_bytes,
                        "dropping websocket message larger than configured max"
                    );
                    continue;
                }
                if let Err(error) = process_text_message(&state, text.as_ref()).await {
                    warn!(error = %error, "failed to process websocket message");
                }
            }
            Ok(Message::Ping(payload)) => {
                debug!(
                    bytes = payload.len(),
                    "received websocket ping from extension"
                );
            }
            Ok(Message::Pong(payload)) => {
                debug!(
                    bytes = payload.len(),
                    "received websocket pong from extension"
                );
            }
            Ok(Message::Close(frame)) => {
                info!(frame = ?frame, "extension websocket closed");
                break;
            }
            Ok(Message::Binary(bin)) => {
                warn!(
                    bytes = bin.len(),
                    "ignoring unexpected binary websocket message"
                );
            }
            Err(error) => {
                error!(error = %error, "websocket read error");
                break;
            }
        }
    }

    state.clear_extension_sender().await;
    writer_handle.abort();
}

async fn process_text_message(state: &Arc<AppState>, raw: &str) -> Result<(), String> {
    state.record_message().await;
    let message: Value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    match classify_incoming(&message) {
        IncomingKind::Hello => {
            state.cache_hello(message.clone()).await;
            if let Some(payload) = message.get("payload") {
                if let Some(windows) = payload.get("windows") {
                    state.cache_windows(windows.clone()).await;
                }
                if let Some(tabs) = payload.get("tabs") {
                    state.cache_tabs(tabs.clone()).await;
                }
            }
            info!("received extension hello message");
        }
        IncomingKind::Event => {
            state.push_event(message.clone()).await;
            debug!("received extension event");
        }
        IncomingKind::Log => {
            debug!(payload = %message, "received extension log message");
        }
        IncomingKind::Response => {
            let id = message
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| "response message missing id".to_string())?
                .to_string();
            state.resolve_pending(&id, message);
        }
        IncomingKind::Keepalive => {
            debug!(payload = %message, "received extension keepalive message");
        }
        IncomingKind::Unknown => {
            warn!(payload = %message, "received unknown extension message");
        }
    }
    Ok(())
}
