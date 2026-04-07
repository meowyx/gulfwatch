use std::collections::HashSet;

use axum::{
    Router,
    extract::{State, WebSocketUpgrade, ws::{Message, WebSocket}},
    response::Response,
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use gulfwatch_core::AppState;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{info, warn};

pub fn ws_routes() -> Router<AppState> {
    Router::new().route("/ws/feed", get(ws_upgrade))
}

async fn ws_upgrade(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ClientMessage {
    Subscribe { subscribe: Vec<String> },
    Unsubscribe { unsubscribe: Vec<String> },
}

#[derive(Serialize)]
struct WsEvent {
    #[serde(rename = "type")]
    event_type: String,
    data: serde_json::Value,
}

async fn handle_ws(socket: WebSocket, state: AppState) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut tx_rx = state.tx_broadcast.subscribe();
    let mut alert_rx = state.alert_broadcast.subscribe();

    // Programs this client is subscribed to. Empty = all programs.
    let mut subscribed: HashSet<String> = HashSet::new();
    let mut subscribe_all = true;

    info!("WebSocket client connected");

    loop {
        tokio::select! {
            // Incoming message from client (subscribe/unsubscribe)
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(ClientMessage::Subscribe { subscribe }) => {
                                subscribe_all = false;
                                for pid in subscribe {
                                    subscribed.insert(pid);
                                }
                            }
                            Ok(ClientMessage::Unsubscribe { unsubscribe }) => {
                                for pid in &unsubscribe {
                                    subscribed.remove(pid);
                                }
                                if subscribed.is_empty() {
                                    subscribe_all = true;
                                }
                            }
                            Err(_) => {
                                warn!("Invalid WebSocket message from client");
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        info!("WebSocket client disconnected");
                        break;
                    }
                    _ => {}
                }
            }

            // Broadcast transaction from the processing pipeline
            result = tx_rx.recv() => {
                match result {
                    Ok(tx) => {
                        if !subscribe_all && !subscribed.contains(&tx.program_id) {
                            continue;
                        }

                        let event = WsEvent {
                            event_type: "transaction".to_string(),
                            data: serde_json::to_value(&tx).unwrap(),
                        };

                        let msg = serde_json::to_string(&event).unwrap();
                        if ws_tx.send(Message::Text(msg.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("WebSocket client lagged, skipped {n} tx messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            // Broadcast alert events
            result = alert_rx.recv() => {
                match result {
                    Ok(alert) => {
                        if !subscribe_all && !subscribed.contains(&alert.program_id) {
                            continue;
                        }

                        let event = WsEvent {
                            event_type: "alert".to_string(),
                            data: serde_json::to_value(&alert).unwrap(),
                        };

                        let msg = serde_json::to_string(&event).unwrap();
                        if ws_tx.send(Message::Text(msg.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("WebSocket client lagged, skipped {n} alert messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}
