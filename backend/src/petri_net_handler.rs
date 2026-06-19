//! Petri net handler.
//!
//! Port of `src/implementation/petri_net_handler.py`.
//!
//! Builds a [`PetriNet`] from a JSON structure, then repeatedly chooses and
//! fires transitions, updating IO outputs and emitting `Deadlock` /
//! `CycleFinished` events. In the live application a background thread runs the
//! step loop; tests can also drive [`PetriNetHandler::step`] directly, exactly
//! like the Python `_step` tests.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Deserialize;

use crate::io_handler::IOHandler;
use crate::petri_net_subcomponents::{
    NodeId, PetriNet, PetriNetError, Place, TransitionsCollection,
};

/// Events emitted by the handler (port of `AbstractPetriNetHandler.Events`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Deadlock,
    CycleFinished,
}

pub use protocol::{DebugInfo, EnablingState};

pub type EventCallback = Box<dyn Fn(Event) + Send + Sync>;
pub type DebugCallback = Box<dyn Fn(DebugInfo) + Send + Sync>;

// ---- JSON structures (the IOPT format) ------------------------------------

#[derive(Debug, Deserialize)]
pub struct PlaceSpec {
    pub id: String,
    pub capacity: i64,
    pub initial_marking: i64,
}

#[derive(Debug, Deserialize)]
pub struct InstantaneousSpec {
    pub id: String,
    pub rate: f64,
    pub priority: i64,
    #[serde(default = "default_true_expr")]
    pub signal_enabling_expression: String,
}

#[derive(Debug, Deserialize)]
pub struct TimedSpec {
    pub id: String,
    pub rate: f64,
    pub priority: i64,
    #[serde(default = "default_true_expr")]
    pub signal_enabling_expression: String,
    pub timer_sec: f64,
}

#[derive(Debug, Deserialize)]
pub struct ArcSpec {
    pub id: String,
    pub source: String,
    pub target: String,
    pub weight: i64,
    #[serde(rename = "type", default)]
    pub arc_type: String,
}

#[derive(Debug, Deserialize)]
pub struct PetriNetStructure {
    pub places: Vec<PlaceSpec>,
    #[serde(default)]
    pub instantaneous_transitions: Vec<InstantaneousSpec>,
    #[serde(default)]
    pub timed_transitions: Vec<TimedSpec>,
    #[serde(default)]
    pub arcs: Vec<ArcSpec>,
    #[serde(default)]
    pub marking_to_output_expressions: HashMap<String, String>,
}

fn default_true_expr() -> String {
    "True".to_string()
}

// ---- Handler --------------------------------------------------------------

struct NetState {
    net: PetriNet,
    collection: TransitionsCollection,
    last_fired: Option<usize>,
}

#[derive(Clone)]
pub struct PetriNetHandler {
    io: Arc<dyn IOHandler>,
    state: Arc<Mutex<Option<NetState>>>,
    running: Arc<AtomicBool>,
    event_callback: Arc<Mutex<Option<EventCallback>>>,
    debug_callback: Arc<Mutex<Option<DebugCallback>>>,
}

impl PetriNetHandler {
    pub fn new(io: Arc<dyn IOHandler>) -> Self {
        PetriNetHandler {
            io,
            state: Arc::new(Mutex::new(None)),
            running: Arc::new(AtomicBool::new(false)),
            event_callback: Arc::new(Mutex::new(None)),
            debug_callback: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_event_callback(&self, callback: EventCallback) {
        *self.event_callback.lock() = Some(callback);
    }

    pub fn set_debug_callback(&self, callback: DebugCallback) {
        *self.debug_callback.lock() = Some(callback);
    }

    pub fn running_flag(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn set_running_flag(&self, flag: bool) {
        self.running.store(flag, Ordering::SeqCst);
    }

    pub fn reset_timers(&self) {
        let mut guard = self.state.lock();
        if let Some(st) = guard.as_mut() {
            st.collection.reset_timers(&mut st.net);
        }
    }

    /// Build the Petri net from a parsed JSON value.
    pub fn setup(&self, value: &serde_json::Value) -> Result<(), Box<dyn std::error::Error>> {
        self.set_running_flag(false);
        let structure: PetriNetStructure = serde_json::from_value(value.clone())?;
        self.setup_from_structure(structure)
    }

    pub fn setup_from_structure(
        &self,
        structure: PetriNetStructure,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut net = PetriNet::new();

        for place in &structure.places {
            net.add_place(Place::new(
                &place.id,
                place.capacity,
                place.initial_marking,
            )?);
        }

        // Valid tokens for boolean expressions = digital inputs + places.
        let mut valid_tokens: Vec<String> =
            self.io.get_all().digital_inputs.keys().cloned().collect();
        valid_tokens.extend(structure.places.iter().map(|p| p.id.clone()));

        self.io.set_marking_to_output_expressions(
            &structure.marking_to_output_expressions,
            &valid_tokens,
        )?;

        let mut transition_indices = Vec::new();
        for t in &structure.instantaneous_transitions {
            let idx = net.add_instantaneous_transition(
                &t.id,
                t.rate,
                t.priority,
                &t.signal_enabling_expression,
                &valid_tokens,
            )?;
            transition_indices.push(idx);
        }
        for t in &structure.timed_transitions {
            let idx = net.add_timed_transition(
                &t.id,
                t.rate,
                t.priority,
                &t.signal_enabling_expression,
                t.timer_sec,
                &valid_tokens,
            )?;
            transition_indices.push(idx);
        }

        for arc in &structure.arcs {
            let source = net.node_by_id(&arc.source).ok_or_else(|| {
                PetriNetError::InvalidArc(format!("unknown source {}", arc.source))
            })?;
            let target = net.node_by_id(&arc.target).ok_or_else(|| {
                PetriNetError::InvalidArc(format!("unknown target {}", arc.target))
            })?;
            net.add_arc(
                &arc.id,
                source,
                target,
                arc.weight,
                arc.arc_type == "inhibitor",
            )?;
        }

        let collection = TransitionsCollection::new(&net, &transition_indices);

        // Compute initial outputs.
        let places_bool = Self::places_bool(&net);
        self.io.update_outputs(&places_bool);

        *self.state.lock() = Some(NetState {
            net,
            collection,
            last_fired: None,
        });
        Ok(())
    }

    fn places_bool(net: &PetriNet) -> HashMap<String, bool> {
        // Lowercased to match the lowercased place tokens in output expressions
        // (the generated closures no longer lowercase per call).
        net.places
            .iter()
            .map(|p| (p.id.to_lowercase(), p.marking() > 0))
            .collect()
    }

    /// Execute a single step (port of `_step`).
    pub fn step(&self) {
        let mut guard = self.state.lock();
        let st = match guard.as_mut() {
            Some(s) => s,
            None => return,
        };

        // Lowercase once per step: signal expressions reference lowercased
        // tokens, and the generated closures no longer lowercase per call.
        let inputs: HashMap<String, bool> = self
            .io
            .get_all()
            .digital_inputs
            .into_iter()
            .map(|(k, v)| (k.to_lowercase(), v))
            .collect();
        let io_updated = self.io.has_been_updated();

        let mut emitted_event: Option<Event> = None;

        match st
            .collection
            .get_transition_chosen_to_fire(&mut st.net, &inputs, io_updated)
        {
            Ok(Some(t)) => {
                st.last_fired = Some(t);
                st.net.fire(t).expect("chosen transition must be fireable");
                let places_bool = Self::places_bool(&st.net);
                self.io.update_outputs(&places_bool);

                let cycle_finished = st
                    .net
                    .places
                    .iter()
                    .all(|p| p.marking() == p.initial_marking());
                if cycle_finished {
                    emitted_event = Some(Event::CycleFinished);
                }
            }
            Ok(None) => {
                st.last_fired = None;
            }
            Err(PetriNetError::Deadlock) => {
                emitted_event = Some(Event::Deadlock);
            }
            Err(_) => {}
        }

        let debug_info = Self::build_debug_info(st);
        drop(guard);

        if let Some(event) = emitted_event {
            if let Some(cb) = self.event_callback.lock().as_ref() {
                cb(event);
            }
        }
        if let Some(cb) = self.debug_callback.lock().as_ref() {
            cb(debug_info);
        }
    }

    fn build_debug_info(st: &NetState) -> DebugInfo {
        let places_marking = Some(
            st.net
                .places
                .iter()
                .map(|p| (p.id.clone(), p.marking()))
                .collect(),
        );

        let transitions_enabling_state = st
            .collection
            .all_transitions()
            .into_iter()
            .map(|idx| {
                let t = &st.net.transitions[idx];
                (
                    t.id.clone(),
                    EnablingState {
                        is_petri_enabled: t.is_petri_enabled_val,
                        is_signal_enabled: t.is_signal_enabled_val,
                    },
                )
            })
            .collect();

        let fired_transition = st.last_fired.map(|idx| st.net.transitions[idx].id.clone());

        DebugInfo {
            places_marking,
            transitions_enabling_state,
            fired_transition,
        }
    }

    /// Spawn the background run loop (port of the Python run thread).
    pub fn spawn_run_loop(&self) {
        let handler = self.clone();
        thread::spawn(move || loop {
            if handler.running.load(Ordering::SeqCst) {
                handler.step();
                thread::sleep(Duration::from_millis(1));
            } else {
                thread::sleep(Duration::from_millis(10));
            }
        });
    }

    /// Read-only snapshot of place markings (used by the web layer).
    pub fn places_marking(&self) -> Option<BTreeMap<String, i64>> {
        self.state.lock().as_ref().map(|st| {
            st.net
                .places
                .iter()
                .map(|p| (p.id.clone(), p.marking()))
                .collect()
        })
    }

    // Convenience for tests, mirrors NodeId usage.
    #[allow(dead_code)]
    fn place_node(net: &PetriNet, id: &str) -> Option<NodeId> {
        net.place_id(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io_handler::IOWebMocker;
    use serde_json::json;
    use std::thread::sleep;

    fn mocker(inputs: &[(&str, bool)], outputs: &[(&str, bool)]) -> Arc<IOWebMocker> {
        let inputs: BTreeMap<String, bool> =
            inputs.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        let outputs: BTreeMap<String, bool> =
            outputs.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        Arc::new(IOWebMocker::new(inputs, outputs))
    }

    #[test]
    fn test_output_update() {
        let io = mocker(&[("i0", false)], &[("o0", false)]);
        let handler = PetriNetHandler::new(io.clone());
        let structure = json!({
            "places": [{"id": "P0", "capacity": 3, "initial_marking": 1}],
            "instantaneous_transitions": [
                {"id": "T0", "rate": 1, "priority": 0, "signal_enabling_expression": "true"}
            ],
            "timed_transitions": [],
            "arcs": [
                {"id": "p0->t0", "source": "P0", "target": "T0", "weight": 1, "type": "normal"}
            ],
            "marking_to_output_expressions": {"o0": "P0"}
        });
        handler.set_event_callback(Box::new(|_| {}));
        handler.setup(&structure).unwrap();
        assert!(io.get_all().digital_outputs["o0"]);
        handler.step();
        assert!(!io.get_all().digital_outputs["o0"]);
    }

    #[test]
    fn test_timed_transitions_01() {
        let io = mocker(
            &[("di0", false), ("di1", false)],
            &[("do0", false), ("do1", false)],
        );
        let handler = PetriNetHandler::new(io.clone());
        let structure = json!({
            "places": [
                {"id": "P0", "initial_marking": 1, "capacity": 0},
                {"id": "P1", "initial_marking": 0, "capacity": 0}
            ],
            "instantaneous_transitions": [
                {"id": "T0", "rate": 1, "priority": 1, "signal_enabling_expression": "di0"}
            ],
            "timed_transitions": [
                {"id": "T1", "rate": 1, "priority": 1, "signal_enabling_expression": "di1", "timer_sec": 1}
            ],
            "arcs": [
                {"id": "P0 to T0", "source": "P0", "target": "T0", "weight": 1, "type": "normal"},
                {"id": "P1 to T1", "source": "P1", "target": "T1", "weight": 1, "type": "normal"},
                {"id": "T0 to P1", "source": "T0", "target": "P1", "weight": 1, "type": "normal"},
                {"id": "T1 to P0", "source": "T1", "target": "P0", "weight": 1, "type": "normal"}
            ],
            "marking_to_output_expressions": {"do0": "P0", "do1": "P1"}
        });
        handler.set_event_callback(Box::new(|_| {}));
        handler.setup(&structure).unwrap();

        let markings = |h: &PetriNetHandler| h.places_marking().unwrap();
        assert_eq!(markings(&handler)["P0"], 1);
        assert_eq!(markings(&handler)["P1"], 0);

        handler.step();
        assert_eq!(markings(&handler)["P0"], 1);
        assert_eq!(markings(&handler)["P1"], 0);

        io.set_input("di0", true);
        handler.step();
        assert_eq!(markings(&handler)["P0"], 0);
        assert_eq!(markings(&handler)["P1"], 1);

        // T1 is timed (1s); without enough time it must not fire.
        io.set_input("di0", false);
        io.set_input("di1", true);
        handler.step();
        assert_eq!(markings(&handler)["P1"], 1);
        sleep(Duration::from_millis(600));
        handler.step();
        assert_eq!(markings(&handler)["P1"], 1);

        // Toggling di1 off resets the timer.
        io.set_input("di1", false);
        handler.step();
        io.set_input("di1", true);
        handler.step();
        sleep(Duration::from_millis(600));
        handler.step();
        assert_eq!(markings(&handler)["P1"], 1);

        sleep(Duration::from_millis(500));
        handler.step();
        assert_eq!(markings(&handler)["P0"], 1);
        assert_eq!(markings(&handler)["P1"], 0);
    }

    #[test]
    fn test_deadlock_event() {
        let io = mocker(&[], &[("o0", false)]);
        let handler = PetriNetHandler::new(io);
        let fired = Arc::new(AtomicBool::new(false));
        let fired_clone = fired.clone();
        handler.set_event_callback(Box::new(move |e| {
            if e == Event::Deadlock {
                fired_clone.store(true, Ordering::SeqCst);
            }
        }));
        let structure = json!({
            "places": [{"id": "P0", "capacity": 1, "initial_marking": 0}],
            "instantaneous_transitions": [
                {"id": "T0", "rate": 1, "priority": 1, "signal_enabling_expression": "true"}
            ],
            "timed_transitions": [],
            "arcs": [
                {"id": "p->t", "source": "P0", "target": "T0", "weight": 1, "type": "normal"}
            ],
            "marking_to_output_expressions": {"o0": "P0"}
        });
        handler.setup(&structure).unwrap();
        handler.step();
        assert!(fired.load(Ordering::SeqCst));
    }
}
