//! Shared wire protocol between the backend and the Leptos WASM frontend.
//!
//! The original Python app used socket.io with a handful of named events. This
//! rewrite replaces it with native WebSockets carrying JSON-encoded, internally
//! tagged enums — the same information, but with a single typed channel shared
//! by both sides so the contract cannot drift.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Digital IO snapshot (port of the Python `{"digital_inputs":..,"digital_outputs":..}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct IoState {
    pub digital_inputs: BTreeMap<String, bool>,
    pub digital_outputs: BTreeMap<String, bool>,
}

/// Per-transition enabling flags for the debug canvas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnablingState {
    pub is_petri_enabled: bool,
    pub is_signal_enabled: bool,
}

/// Live Petri-net debugging info (port of the `petrinet_debugging_info` event).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DebugInfo {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub places_marking: Option<BTreeMap<String, i64>>,
    pub transitions_enabling_state: BTreeMap<String, EnablingState>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fired_transition: Option<String>,
}

/// Which IO module is active and whether the physical one is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IoModuleSelected {
    pub is_physical_io_module: bool,
    pub is_physical_io_module_enabled: bool,
}

/// Messages sent from the server to the browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ServerMsg {
    /// Replaces socket.io `IO_update`.
    IoUpdate(IoState),
    /// Replaces socket.io `stateMachine_state_update`. Carries the state name.
    StateUpdate(String),
    /// Replaces socket.io `io_module_selected`.
    IoModuleSelected(IoModuleSelected),
    /// Replaces socket.io `petrinet_json_update`. Carries the full IOPT dict.
    PetrinetJson(serde_json::Value),
    /// Replaces socket.io `petrinet_debugging_info`.
    PetrinetDebuggingInfo(DebugInfo),
}

/// Messages sent from the browser to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClientMsg {
    /// Replaces socket.io `stateMachine_event_update`. Carries the button id,
    /// e.g. `btn-start`, `FileUploaded`, `io_handler_emulator`.
    StateMachineEvent(String),
    /// Replaces socket.io `IOPT_update`. Carries the IOPT dict as a JSON string.
    IoptUpdate(String),
    /// Replaces the IO-mocker socket.io `input_update`. Toggles a single input.
    InputUpdate { id: String, value: bool },
}

/// The control buttons / events recognised by the state machine.
/// Mirrors `LocalWebServer._command_dictionary` in the Python code.
pub mod commands {
    pub const BTN_START: &str = "btn-start";
    pub const BTN_PAUSE: &str = "btn-pause";
    pub const BTN_RESUME: &str = "btn-resume";
    pub const BTN_FINISH: &str = "btn-finish";
    pub const BTN_FINISH_NOW: &str = "btn-finish_now";
    pub const FILE_UPLOADED: &str = "FileUploaded";
    pub const IO_HANDLER_PHYSICAL: &str = "io_handler_physical";
    pub const IO_HANDLER_EMULATOR: &str = "io_handler_emulator";
}
