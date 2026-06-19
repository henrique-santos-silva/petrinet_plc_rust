//! End-to-end integration test for the web interface.
//!
//! Spins up the full stack (IO emulator + Petri-net handler + state machine +
//! axum web server) on an ephemeral port, then drives it like a browser would:
//! connects over the native WebSocket, uploads a Petri net, starts execution,
//! toggles a digital input and verifies the outputs and state react — i.e. the
//! same interactive flow a user performs in the UI.

use std::collections::BTreeMap;
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use petrinet_plc::io_handler::{IOHandler, IOWebMocker};
use petrinet_plc::petri_net_handler::PetriNetHandler;
use petrinet_plc::state_machine::StateMachine;
use petrinet_plc::webserver_handler::WebServerHandler;
use protocol::{ClientMsg, ServerMsg};

/// Start the whole application on 127.0.0.1:0 and return the bound address.
async fn start_app() -> String {
    let digital_inputs: BTreeMap<String, bool> =
        (0..8).map(|i| (format!("DI{i}"), false)).collect();
    let digital_outputs: BTreeMap<String, bool> =
        (0..16).map(|i| (format!("DO{i}"), false)).collect();
    let io: Arc<dyn IOHandler> = Arc::new(IOWebMocker::new(digital_inputs, digital_outputs));

    // Unique temp file for the persisted IOPT.
    let mut iopt_path = std::env::temp_dir();
    iopt_path.push(format!(
        "petrinet_test_iopt_{}_{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&iopt_path);

    let webserver = WebServerHandler::new(io.clone(), iopt_path);
    let petrinet = PetriNetHandler::new(io.clone());

    let (web_tx, web_rx) = channel();
    let (net_tx, net_rx) = channel();
    webserver.set_event_sender(web_tx);

    {
        let net_tx = net_tx.clone();
        petrinet.set_event_callback(Box::new(move |event| {
            let _ = net_tx.send(event);
        }));
    }
    {
        let webserver = webserver.clone();
        petrinet.set_debug_callback(Box::new(move |info| {
            webserver.post_current_petrinet_debugging_info(info);
        }));
    }
    petrinet.spawn_run_loop();

    let state_machine = StateMachine::new(
        petrinet.clone(),
        webserver.clone(),
        io.clone(),
        web_rx,
        net_rx,
    );
    std::thread::spawn(move || state_machine.run());

    let app = webserver.router(std::env::temp_dir());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("{addr}")
}

fn demo_net() -> serde_json::Value {
    // Two places in a loop; T0 (di0) moves the token P0->P1, T1 (di1) back.
    // Outputs DO0/DO1 mirror P0/P1.
    serde_json::json!({
        "places": [
            {"id": "P0", "capacity": 1, "initial_marking": 1},
            {"id": "P1", "capacity": 1, "initial_marking": 0}
        ],
        "instantaneous_transitions": [
            {"id": "T0", "rate": 1, "priority": 1, "signal_enabling_expression": "di0"},
            {"id": "T1", "rate": 1, "priority": 1, "signal_enabling_expression": "di1"}
        ],
        "timed_transitions": [],
        "arcs": [
            {"id": "a0", "source": "P0", "target": "T0", "weight": 1, "type": "normal"},
            {"id": "a1", "source": "T0", "target": "P1", "weight": 1, "type": "normal"},
            {"id": "a2", "source": "P1", "target": "T1", "weight": 1, "type": "normal"},
            {"id": "a3", "source": "T1", "target": "P0", "weight": 1, "type": "normal"}
        ],
        "marking_to_output_expressions": {"DO0": "P0", "DO1": "P1"}
    })
}

#[tokio::test]
async fn test_web_interface_interactive_flow() {
    let addr = start_app().await;
    let url = format!("ws://{addr}/ws");

    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("websocket should connect");

    // Read messages continuously until `predicate` matches one, or the overall
    // `deadline_secs` elapses. Reading against a single deadline (rather than a
    // fixed per-message timeout inside a loop) keeps the test fast: it returns
    // as soon as the awaited message arrives and never multiplies timeouts.
    async fn wait_until<F>(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        deadline_secs: u64,
        predicate: F,
    ) -> bool
    where
        F: Fn(&ServerMsg) -> bool,
    {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(deadline_secs);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match tokio::time::timeout(remaining, ws.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    if let Ok(msg) = serde_json::from_str::<ServerMsg>(&text) {
                        if predicate(&msg) {
                            return true;
                        }
                    }
                }
                Ok(Some(Ok(_))) => continue,
                // stream ended, errored, or overall deadline hit
                _ => return false,
            }
        }
    }

    // 1. We must receive an initial state update (the machine starts in
    //    CheckingPetriNetFilesExistence / WaitingPetriNetFilesUpload).
    let saw_state = wait_until(&mut ws, 10, |msg| {
        matches!(
            msg,
            ServerMsg::StateUpdate(name)
                if ["INIT", "CheckingPetriNetFilesExistence", "WaitingPetriNetFilesUpload"]
                    .contains(&name.as_str())
        )
    })
    .await;
    assert!(saw_state, "did not receive an initial state update");

    // 2. Upload a Petri net (browser sends IOPT_update then FileUploaded).
    let net = demo_net();
    ws.send(Message::Text(
        serde_json::to_string(&ClientMsg::IoptUpdate(net.to_string())).unwrap(),
    ))
    .await
    .unwrap();
    ws.send(Message::Text(
        serde_json::to_string(&ClientMsg::StateMachineEvent("FileUploaded".into())).unwrap(),
    ))
    .await
    .unwrap();

    // 3. Start execution.
    ws.send(Message::Text(
        serde_json::to_string(&ClientMsg::StateMachineEvent("btn-start".into())).unwrap(),
    ))
    .await
    .unwrap();

    // Expect a transition to the Running state.
    let saw_running = wait_until(
        &mut ws,
        10,
        |msg| matches!(msg, ServerMsg::StateUpdate(name) if name == "Running"),
    )
    .await;
    assert!(saw_running, "machine never reported Running");

    // 4. Toggle DI0 -> T0 fires -> P1 gets the token -> DO1 becomes true.
    ws.send(Message::Text(
        serde_json::to_string(&ClientMsg::InputUpdate {
            id: "DI0".into(),
            value: true,
        })
        .unwrap(),
    ))
    .await
    .unwrap();

    let saw_do1 = wait_until(&mut ws, 10, |msg| {
        matches!(
            msg,
            ServerMsg::IoUpdate(io)
                if io.digital_outputs.get("DO1") == Some(&true)
                    && io.digital_outputs.get("DO0") == Some(&false)
        )
    })
    .await;
    assert!(
        saw_do1,
        "output DO1 never turned on after toggling input DI0"
    );
}

#[tokio::test]
async fn test_getfile_endpoint_404_when_absent() {
    let addr = start_app().await;
    let url = format!("http://{addr}/api/getFile/IOPT.json");
    let resp = reqwest::get(&url).await.unwrap();
    // No file uploaded yet for this fresh instance.
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}
