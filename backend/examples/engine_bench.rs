//! Engine throughput benchmark (Rust side).
//!
//! Builds a deterministic P0 <-> P1 cycle with two instantaneous transitions
//! that fire on every step, then measures transition firings per second.
//! Run with: `cargo run --release --example engine_bench -- [iterations]`
//!
//! This mirrors `bench/py_engine_bench.py` exactly so the two numbers are
//! comparable: same net, same hot path (choose-to-fire + fire), no web layer.

use std::collections::HashMap;
use std::time::Instant;

use petrinet_plc::petri_net_subcomponents::{NodeId, PetriNet, Place, TransitionsCollection};

fn main() {
    let iterations: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000_000);
    // mode: "plain" (signal = True) or "signal" (transitions guarded by inputs)
    let mode = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "plain".to_string());

    let mut net = PetriNet::new();
    let p0 = net.add_place(Place::new("P0", 1, 1).unwrap());
    let p1 = net.add_place(Place::new("P1", 1, 0).unwrap());

    let valid = ["DI0".to_string(), "DI1".to_string()];
    let (expr0, expr1): (&str, &str) = if mode == "signal" {
        ("di0", "di1")
    } else {
        ("True", "True")
    };
    let t0 = net
        .add_instantaneous_transition("T0", 1.0, 1, expr0, &valid)
        .unwrap();
    let t1 = net
        .add_instantaneous_transition("T1", 1.0, 1, expr1, &valid)
        .unwrap();
    net.add_arc("a0", NodeId::Place(p0), NodeId::Transition(t0), 1, false)
        .unwrap();
    net.add_arc("a1", NodeId::Transition(t0), NodeId::Place(p1), 1, false)
        .unwrap();
    net.add_arc("a2", NodeId::Place(p1), NodeId::Transition(t1), 1, false)
        .unwrap();
    net.add_arc("a3", NodeId::Transition(t1), NodeId::Place(p0), 1, false)
        .unwrap();

    let mut coll_transitions = vec![t0, t1];

    // "wide" mode: add K idle transitions (each guarded by an empty place so it
    // never fires). They exercise the per-step enabling scan: with the full
    // rescan their arcs are walked every step; with incremental enabling they
    // are computed once and then stay clean.
    if mode == "wide" {
        let k: usize = std::env::args()
            .nth(3)
            .and_then(|s| s.parse().ok())
            .unwrap_or(2000);
        for i in 0..k {
            let pi = net.add_place(Place::new(&format!("PI{i}"), 1, 0).unwrap());
            let ti = net
                .add_instantaneous_transition(&format!("TI{i}"), 1.0, 1, "True", &[])
                .unwrap();
            net.add_arc(
                &format!("ai{i}"),
                NodeId::Place(pi),
                NodeId::Transition(ti),
                1,
                false,
            )
            .unwrap();
            coll_transitions.push(ti);
        }
    }

    let mut coll = TransitionsCollection::new(&net, &coll_transitions);
    // Inputs are passed pre-lowercased (as the production caller does once/step).
    let mut inputs: HashMap<String, bool> = HashMap::new();
    if mode == "signal" {
        inputs.insert("di0".to_string(), true);
        inputs.insert("di1".to_string(), true);
    }

    let mut firings: u64 = 0;
    let start = Instant::now();
    for _ in 0..iterations {
        match coll.get_transition_chosen_to_fire(&mut net, &inputs, false) {
            Ok(Some(t)) => {
                net.fire(t).unwrap();
                firings += 1;
            }
            Ok(None) => {}
            Err(e) => panic!("unexpected: {e}"),
        }
    }
    let elapsed = start.elapsed();

    let secs = elapsed.as_secs_f64();
    println!("RUST engine benchmark (mode={mode})");
    println!("  iterations : {iterations}");
    println!("  firings    : {firings}");
    println!("  elapsed    : {secs:.4} s");
    println!("  firings/s  : {:.0}", firings as f64 / secs);
}
