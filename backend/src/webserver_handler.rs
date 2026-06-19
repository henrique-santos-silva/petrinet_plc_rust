//! Web server handler.
//!
//! Port of `src/implementation/webserver_handler.py` (the `LocalWebServer`).
//!
//! The Python version used Flask + socket.io. This rewrite uses axum with a
//! single native WebSocket endpoint (`/ws`) carrying the typed [`protocol`]
//! messages, plus static file serving for the Leptos WASM frontend and the
//! `GET /api/getFile/IOPT.json` download endpoint.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

use protocol::{commands, ClientMsg, DebugInfo, IoModuleSelected, IoState, ServerMsg};

use crate::io_handler::IOHandler;

/// Events the web UI can raise (port of `AbstractWebServerHandler.Events`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebServerEvent {
    PetriNetFilesUploaded,
    StartExecution,
    PauseExecution,
    ResumeExecution,
    FinishExecutionAfterCycle,
    FinishExecutionImmediately,
    PhysicalIOHandlerSelected,
    EmulatorIOHandlerSelected,
}

fn command_to_event(button_id: &str) -> Option<WebServerEvent> {
    match button_id {
        commands::BTN_START => Some(WebServerEvent::StartExecution),
        commands::BTN_PAUSE => Some(WebServerEvent::PauseExecution),
        commands::BTN_RESUME => Some(WebServerEvent::ResumeExecution),
        commands::BTN_FINISH => Some(WebServerEvent::FinishExecutionAfterCycle),
        commands::BTN_FINISH_NOW => Some(WebServerEvent::FinishExecutionImmediately),
        commands::FILE_UPLOADED => Some(WebServerEvent::PetriNetFilesUploaded),
        commands::IO_HANDLER_PHYSICAL => Some(WebServerEvent::PhysicalIOHandlerSelected),
        commands::IO_HANDLER_EMULATOR => Some(WebServerEvent::EmulatorIOHandlerSelected),
        _ => None,
    }
}

struct Inner {
    broadcast: broadcast::Sender<ServerMsg>,
    event_tx: Mutex<Option<Sender<WebServerEvent>>>,
    io: Arc<dyn IOHandler>,
    iopt_path: PathBuf,
    iopt: Mutex<Option<serde_json::Value>>,
    current_state: Mutex<Option<String>>,
    current_io: Mutex<Option<IoState>>,
    current_places_marking: Mutex<Option<std::collections::BTreeMap<String, i64>>>,
    current_enabling: Mutex<Option<std::collections::BTreeMap<String, protocol::EnablingState>>>,
    io_module: Mutex<IoModuleSelected>,
}

/// Cloneable handle to the web server (shared `Arc` inside).
#[derive(Clone)]
pub struct WebServerHandler {
    inner: Arc<Inner>,
}

impl WebServerHandler {
    pub fn new(io: Arc<dyn IOHandler>, iopt_path: PathBuf) -> Self {
        let (broadcast, _) = broadcast::channel(256);
        if let Some(parent) = iopt_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        WebServerHandler {
            inner: Arc::new(Inner {
                broadcast,
                event_tx: Mutex::new(None),
                io,
                iopt_path,
                iopt: Mutex::new(None),
                current_state: Mutex::new(None),
                current_io: Mutex::new(None),
                current_places_marking: Mutex::new(None),
                current_enabling: Mutex::new(None),
                io_module: Mutex::new(IoModuleSelected {
                    is_physical_io_module: true,
                    is_physical_io_module_enabled: true,
                }),
            }),
        }
    }

    /// Wire the channel the state machine reads its web events from.
    pub fn set_event_sender(&self, tx: Sender<WebServerEvent>) {
        *self.inner.event_tx.lock() = Some(tx);
    }

    pub fn check_petri_net_files_existence(&self) -> bool {
        self.inner.iopt_path.is_file()
    }

    /// Read back the persisted IOPT json (port of `get_file`).
    pub fn get_file(&self) -> std::io::Result<serde_json::Value> {
        let content = std::fs::read_to_string(&self.inner.iopt_path)?;
        let value = serde_json::from_str(&content)?;
        Ok(value)
    }

    // ---- "post_*" methods: update cache + broadcast --------------------

    pub fn post_ios(&self, ios: IoState) {
        let mut current = self.inner.current_io.lock();
        if current.as_ref() != Some(&ios) {
            *current = Some(ios.clone());
            drop(current);
            let _ = self.inner.broadcast.send(ServerMsg::IoUpdate(ios));
        }
    }

    pub fn post_state(&self, state_name: &str) {
        let mut current = self.inner.current_state.lock();
        if current.as_deref() != Some(state_name) {
            *current = Some(state_name.to_string());
            drop(current);
            let _ = self
                .inner
                .broadcast
                .send(ServerMsg::StateUpdate(state_name.to_string()));
        }
    }

    pub fn post_current_io_module(
        &self,
        is_physical: Option<bool>,
        is_physical_enabled: Option<bool>,
    ) {
        let module = {
            let mut module = self.inner.io_module.lock();
            if let Some(v) = is_physical {
                module.is_physical_io_module = v;
            }
            if let Some(v) = is_physical_enabled {
                module.is_physical_io_module_enabled = v;
            }
            *module
        };
        let _ = self
            .inner
            .broadcast
            .send(ServerMsg::IoModuleSelected(module));
    }

    /// Port of `post_current_petrinet_debugging_info`.
    pub fn post_current_petrinet_debugging_info(&self, info: DebugInfo) {
        if info.fired_transition.is_some() {
            *self.inner.current_places_marking.lock() = info.places_marking.clone();
            *self.inner.current_enabling.lock() = Some(info.transitions_enabling_state.clone());
            let _ = self
                .inner
                .broadcast
                .send(ServerMsg::PetrinetDebuggingInfo(info));
        } else {
            let mut current = self.inner.current_enabling.lock();
            if current.as_ref() != Some(&info.transitions_enabling_state) {
                *current = Some(info.transitions_enabling_state.clone());
                drop(current);
                let _ = self
                    .inner
                    .broadcast
                    .send(ServerMsg::PetrinetDebuggingInfo(DebugInfo {
                        places_marking: None,
                        transitions_enabling_state: info.transitions_enabling_state,
                        fired_transition: None,
                    }));
            }
        }
    }

    // ---- WebSocket handling -------------------------------------------

    fn handle_client_msg(&self, msg: ClientMsg) {
        match msg {
            ClientMsg::StateMachineEvent(button_id) => {
                if let Some(event) = command_to_event(&button_id) {
                    if let Some(tx) = self.inner.event_tx.lock().as_ref() {
                        let _ = tx.send(event);
                    }
                }
            }
            ClientMsg::IoptUpdate(json_str) => {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json_str) {
                    if let Ok(pretty) = serde_json::to_string_pretty(&value) {
                        let _ = std::fs::write(&self.inner.iopt_path, pretty);
                    }
                    *self.inner.iopt.lock() = Some(value.clone());
                    // A new net invalidates the cached debug view.
                    *self.inner.current_places_marking.lock() = None;
                    *self.inner.current_enabling.lock() = None;
                    let _ = self.inner.broadcast.send(ServerMsg::PetrinetJson(value));
                }
            }
            ClientMsg::InputUpdate { id, value } => {
                self.inner.io.set_input(&id, value);
            }
        }
    }

    /// Messages sent to a client immediately after it connects (port of
    /// `handle_new_connection`).
    fn initial_messages(&self) -> Vec<ServerMsg> {
        let mut msgs = Vec::new();
        if let Some(state) = self.inner.current_state.lock().clone() {
            msgs.push(ServerMsg::StateUpdate(state));
        }
        if let Some(io) = self.inner.current_io.lock().clone() {
            msgs.push(ServerMsg::IoUpdate(io));
        }
        msgs.push(ServerMsg::IoModuleSelected(*self.inner.io_module.lock()));

        if self.check_petri_net_files_existence() {
            if let Ok(value) = self.get_file() {
                msgs.push(ServerMsg::PetrinetJson(value));
            }
        }
        msgs.push(ServerMsg::PetrinetDebuggingInfo(DebugInfo {
            places_marking: self.inner.current_places_marking.lock().clone(),
            transitions_enabling_state: self
                .inner
                .current_enabling
                .lock()
                .clone()
                .unwrap_or_default(),
            fired_transition: None,
        }));
        msgs
    }

    /// Build the axum router. `static_dir` is the trunk `dist` output.
    pub fn router(&self, static_dir: PathBuf) -> Router {
        // Serve static assets; for unknown paths (the SPA client-side routes
        // like `/iomocker` and `/debug`) fall back to index.html so the Leptos
        // router can render the right view.
        let index = static_dir.join("index.html");
        let serve = ServeDir::new(static_dir)
            .append_index_html_on_directories(true)
            .fallback(ServeFile::new(index));

        Router::new()
            .route("/ws", get(ws_handler))
            .route("/api/getFile/IOPT.json", get(get_iopt_file))
            .fallback_service(serve)
            .layer(CorsLayer::permissive())
            .with_state(self.clone())
    }
}

async fn get_iopt_file(State(handler): State<WebServerHandler>) -> Response {
    match std::fs::read(&handler.inner.iopt_path) {
        Ok(bytes) => (
            [
                ("content-type", "application/json"),
                ("content-disposition", "attachment; filename=\"IOPT.json\""),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (axum::http::StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn ws_handler(ws: WebSocketUpgrade, State(handler): State<WebServerHandler>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, handler))
}

async fn handle_socket(socket: WebSocket, handler: WebServerHandler) {
    let (mut sink, mut stream) = socket.split();
    let mut rx = handler.inner.broadcast.subscribe();

    // Send the initial snapshot.
    for msg in handler.initial_messages() {
        if let Ok(text) = serde_json::to_string(&msg) {
            if sink.send(Message::Text(text)).await.is_err() {
                return;
            }
        }
    }

    // Forward broadcast messages to this client.
    let mut send_task = tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            if let Ok(text) = serde_json::to_string(&msg) {
                if sink.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
        }
    });

    // Receive client messages.
    let recv_handler = handler.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(message)) = stream.next().await {
            if let Message::Text(text) = message {
                if let Ok(client_msg) = serde_json::from_str::<ClientMsg>(&text) {
                    recv_handler.handle_client_msg(client_msg);
                }
            }
        }
    });

    // If either task finishes, abort the other.
    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }
}
