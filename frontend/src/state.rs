//! Global reactive state and the native WebSocket connection.
//!
//! Replaces the per-page socket.io connections of the original frontend with a
//! single typed WebSocket carrying [`protocol`] messages. All views read from
//! the same reactive signals and send [`ClientMsg`]s through the same socket.

use leptos::*;
use protocol::{ClientMsg, DebugInfo, IoModuleSelected, IoState, ServerMsg};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, WebSocket};

#[derive(Clone)]
pub struct AppState {
    pub io_state: RwSignal<IoState>,
    pub machine_state: RwSignal<String>,
    pub io_module: RwSignal<IoModuleSelected>,
    pub petrinet_json: RwSignal<Option<serde_json::Value>>,
    pub debug_info: RwSignal<DebugInfo>,
    pub connected: RwSignal<bool>,
    ws: StoredValue<Option<WebSocket>>,
}

impl AppState {
    pub fn new() -> Self {
        AppState {
            io_state: create_rw_signal(IoState::default()),
            machine_state: create_rw_signal(String::new()),
            io_module: create_rw_signal(IoModuleSelected {
                is_physical_io_module: true,
                is_physical_io_module_enabled: true,
            }),
            petrinet_json: create_rw_signal(None),
            debug_info: create_rw_signal(DebugInfo::default()),
            connected: create_rw_signal(false),
            ws: store_value(None),
        }
    }

    /// Send a typed message to the backend.
    pub fn send(&self, msg: &ClientMsg) {
        if let Ok(text) = serde_json::to_string(msg) {
            self.ws.with_value(|ws| {
                if let Some(ws) = ws {
                    let _ = ws.send_with_str(&text);
                }
            });
        }
    }

    /// Compute the `ws://host/ws` URL from the current page location.
    fn ws_url() -> String {
        let location = web_sys::window().unwrap().location();
        let protocol = location.protocol().unwrap_or_else(|_| "http:".into());
        let host = location.host().unwrap_or_else(|_| "localhost:50000".into());
        let ws_scheme = if protocol == "https:" { "wss" } else { "ws" };
        format!("{ws_scheme}://{host}/ws")
    }

    /// Open the WebSocket and wire incoming messages to the signals.
    pub fn connect(&self) {
        let ws = match WebSocket::new(&Self::ws_url()) {
            Ok(ws) => ws,
            Err(_) => return,
        };

        let state = self.clone();
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
            if let Some(text) = e.data().as_string() {
                if let Ok(msg) = serde_json::from_str::<ServerMsg>(&text) {
                    state.handle_server_msg(msg);
                }
            }
        });
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        on_message.forget();

        let connected = self.connected;
        let on_open = Closure::<dyn FnMut()>::new(move || connected.set(true));
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        on_open.forget();

        let connected = self.connected;
        let on_close = Closure::<dyn FnMut()>::new(move || connected.set(false));
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        on_close.forget();

        self.ws.set_value(Some(ws));
    }

    fn handle_server_msg(&self, msg: ServerMsg) {
        match msg {
            ServerMsg::IoUpdate(io) => self.io_state.set(io),
            ServerMsg::StateUpdate(name) => self.machine_state.set(name),
            ServerMsg::IoModuleSelected(module) => self.io_module.set(module),
            ServerMsg::PetrinetJson(value) => self.petrinet_json.set(Some(value)),
            ServerMsg::PetrinetDebuggingInfo(info) => {
                // Merge: enabling-only updates keep the last known markings.
                self.debug_info.update(|current| {
                    if info.places_marking.is_some() {
                        current.places_marking = info.places_marking.clone();
                    }
                    current.transitions_enabling_state = info.transitions_enabling_state.clone();
                    current.fired_transition = info.fired_transition.clone();
                });
            }
        }
    }
}

/// Convenience accessor for the app state provided via context.
pub fn use_app() -> AppState {
    use_context::<AppState>().expect("AppState context must be provided")
}
