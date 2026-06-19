//! Application entry point.
//!
//! Port of the top-level `run.py`. Wires the IO emulator, Petri-net handler,
//! web server and state machine together, then serves the web UI.
//!
//! Threading model (mirrors the Python threads):
//! - the Petri-net step loop runs on its own background thread,
//! - the state machine runs on its own background thread,
//! - the axum web server runs on the tokio runtime in `main`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::Arc;

use petrinet_plc::io_handler::{IOHandler, IOWebMocker};
use petrinet_plc::petri_net_handler::PetriNetHandler;
use petrinet_plc::state_machine::StateMachine;
use petrinet_plc::webserver_handler::WebServerHandler;

fn static_dir() -> PathBuf {
    // The Leptos frontend is built with trunk into ../frontend/dist.
    if let Ok(dir) = std::env::var("PETRINET_STATIC_DIR") {
        return PathBuf::from(dir);
    }
    let manifest = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest).join("../frontend/dist")
}

fn iopt_path() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest).join("webserver_ihm_uploaded_IOPT/IOPT.json")
}

#[tokio::main]
async fn main() {
    // IO emulator: DI0..7 inputs, DO0..15 outputs (matches the Python run.py).
    let digital_inputs: BTreeMap<String, bool> =
        (0..8).map(|i| (format!("DI{i}"), false)).collect();
    let digital_outputs: BTreeMap<String, bool> =
        (0..16).map(|i| (format!("DO{i}"), false)).collect();
    let io: Arc<dyn IOHandler> = Arc::new(IOWebMocker::new(digital_inputs, digital_outputs));

    let webserver = WebServerHandler::new(io.clone(), iopt_path());
    let petrinet = PetriNetHandler::new(io.clone());

    // Channels: web events and petri-net events feed the state machine.
    let (web_tx, web_rx) = channel();
    let (net_tx, net_rx) = channel();

    webserver.set_event_sender(web_tx);

    // Petri-net events -> state machine.
    {
        let net_tx = net_tx.clone();
        petrinet.set_event_callback(Box::new(move |event| {
            let _ = net_tx.send(event);
        }));
    }

    // Petri-net debug snapshots -> web UI.
    {
        let webserver = webserver.clone();
        petrinet.set_debug_callback(Box::new(move |info| {
            webserver.post_current_petrinet_debugging_info(info);
        }));
    }

    petrinet.spawn_run_loop();

    // State machine thread.
    let state_machine = StateMachine::new(
        petrinet.clone(),
        webserver.clone(),
        io.clone(),
        web_rx,
        net_rx,
    );
    std::thread::spawn(move || state_machine.run());

    // Web server.
    let app = webserver.router(static_dir());
    let addr = "0.0.0.0:50000";
    println!("PetriNet PLC (Rust) listening on http://{addr}");
    println!("Serving frontend from {:?}", static_dir());
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
