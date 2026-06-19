//! Leptos WASM frontend entry point.
//!
//! Single-page app replacing the original three socket.io pages:
//! - `/`         -> IHM control panel
//! - `/iomocker` -> IO emulator (clickable inputs)
//! - `/debug`    -> live Petri-net canvas
//!
//! All views share one native WebSocket connection (see [`state`]).

mod debug_view;
mod ihm;
mod iomocker;
mod iopt;
mod state;

use leptos::*;
use leptos_router::*;

use crate::debug_view::DebugView;
use crate::ihm::Ihm;
use crate::iomocker::IoMocker;
use crate::state::AppState;

#[component]
fn App() -> impl IntoView {
    // Create the shared state, connect the WebSocket, and provide it to all
    // descendant views via context.
    let app_state = AppState::new();
    app_state.connect();
    provide_context(app_state);

    view! {
        <Router>
            <Routes>
                <Route path="/" view=Ihm />
                <Route path="/iomocker" view=IoMocker />
                <Route path="/debug" view=DebugView />
            </Routes>
        </Router>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}
