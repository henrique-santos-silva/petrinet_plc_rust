//! Petri net subcomponents.
//!
//! Port of `src/implementation/petri_net_subcomponents.py`.
//!
//! The Python version models a graph of `Place`/`Transition` nodes connected by
//! `Arc`s, where nodes keep references to the arcs touching them. To reproduce
//! that bidirectional graph safely in Rust we use an index-based arena: the
//! `PetriNet` owns vectors of places, transitions and arcs, and every node
//! stores the indices of the arcs that touch it.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::time::Instant;

use crate::bool_parser::{BoolFn, BoolParser, SyntaxError};

#[derive(Debug, Clone, PartialEq)]
pub enum PetriNetError {
    /// An arc tried to connect two places or two transitions.
    InvalidArc(String),
    /// A place marking exceeded its capacity.
    MarkingExceedsCapacity,
    /// The net reached a deadlock (no transition can ever fire).
    Deadlock,
    /// A signal-enabling expression failed to parse.
    Parse(SyntaxError),
}

impl std::fmt::Display for PetriNetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PetriNetError::InvalidArc(s) => write!(f, "invalid arc: {s}"),
            PetriNetError::MarkingExceedsCapacity => {
                write!(f, "place marking can't be greater than its capacity")
            }
            PetriNetError::Deadlock => write!(f, "Petri net deadlock detected"),
            PetriNetError::Parse(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PetriNetError {}

/// Identifies a node within the arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeId {
    Place(usize),
    Transition(usize),
}

#[derive(Debug, Clone)]
pub struct Place {
    pub id: String,
    /// `None` represents infinite capacity (Python `float('inf')`).
    capacity: Option<i64>,
    marking: i64,
    initial_marking: i64,
    arcs_from_this_node: Vec<usize>,
    arcs_to_this_node: Vec<usize>,
}

impl Place {
    pub fn new(id: &str, capacity: i64, marking: i64) -> Result<Self, PetriNetError> {
        let capacity = if capacity > 0 { Some(capacity) } else { None };
        if let Some(cap) = capacity {
            if marking > cap {
                return Err(PetriNetError::MarkingExceedsCapacity);
            }
        }
        Ok(Place {
            id: id.to_string(),
            capacity,
            marking,
            initial_marking: marking,
            arcs_from_this_node: Vec::new(),
            arcs_to_this_node: Vec::new(),
        })
    }

    pub fn capacity(&self) -> Option<i64> {
        self.capacity
    }

    pub fn marking(&self) -> i64 {
        self.marking
    }

    pub fn initial_marking(&self) -> i64 {
        self.initial_marking
    }

    pub fn set_marking(&mut self, marking: i64) -> Result<(), PetriNetError> {
        if let Some(cap) = self.capacity {
            if marking > cap {
                return Err(PetriNetError::MarkingExceedsCapacity);
            }
        }
        self.marking = marking;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Arc {
    pub id: String,
    pub source: NodeId,
    pub target: NodeId,
    pub weight: i64,
    pub is_inhibitor: bool,
}

/// Whether a transition fires instantly or after a delay.
pub enum TransitionKind {
    Instantaneous,
    Timed {
        timer_sec: f64,
        enabled_since: Option<Instant>,
    },
}

pub struct Transition {
    pub id: String,
    rate: f64,
    priority: i64,
    /// `None` means "always signal-enabled" (the `"True"` expression).
    signal_fn: Option<BoolFn>,
    pub is_signal_enabled_val: bool,
    pub is_petri_enabled_val: bool,
    pub is_time_enabled_val: bool,
    kind: TransitionKind,
    arcs_from_this_node: Vec<usize>,
    arcs_to_this_node: Vec<usize>,
}

impl Transition {
    pub fn rate(&self) -> f64 {
        self.rate
    }

    pub fn priority(&self) -> i64 {
        self.priority
    }

    pub fn is_timed(&self) -> bool {
        matches!(self.kind, TransitionKind::Timed { .. })
    }
}

/// Builds the signal-enabling function from a C++-style expression.
fn build_signal_fn(
    expression: &str,
    valid_extra_tokens: &[String],
) -> Result<Option<BoolFn>, PetriNetError> {
    if expression == "True" {
        return Ok(None);
    }
    let parser = BoolParser::new(expression, valid_extra_tokens).map_err(PetriNetError::Parse)?;
    Ok(Some(parser.generate_function()))
}

/// The arena owning the whole Petri net graph.
#[derive(Default)]
pub struct PetriNet {
    pub places: Vec<Place>,
    pub transitions: Vec<Transition>,
    pub arcs: Vec<Arc>,
    place_index_by_id: HashMap<String, usize>,
    transition_index_by_id: HashMap<String, usize>,
    /// For each place, the transitions whose Petri-enabling depends on that
    /// place's marking (any arc touching it). When a place's marking changes,
    /// only these transitions need their enabling recomputed.
    place_neighbors: Vec<Vec<usize>>,
    /// Per-transition flag: Petri-enabling must be recomputed (a relevant place
    /// changed since the last computation). All transitions start dirty.
    petri_dirty: Vec<bool>,
}

impl PetriNet {
    pub fn new() -> Self {
        PetriNet::default()
    }

    pub fn add_place(&mut self, place: Place) -> usize {
        let idx = self.places.len();
        self.place_index_by_id.insert(place.id.clone(), idx);
        self.places.push(place);
        self.place_neighbors.push(Vec::new());
        idx
    }

    pub fn add_instantaneous_transition(
        &mut self,
        id: &str,
        rate: f64,
        priority: i64,
        signal_enabling_expression: &str,
        valid_extra_tokens: &[String],
    ) -> Result<usize, PetriNetError> {
        let signal_fn = build_signal_fn(signal_enabling_expression, valid_extra_tokens)?;
        let t = Transition {
            id: id.to_string(),
            rate,
            priority,
            signal_fn,
            is_signal_enabled_val: false,
            is_petri_enabled_val: false,
            is_time_enabled_val: false,
            kind: TransitionKind::Instantaneous,
            arcs_from_this_node: Vec::new(),
            arcs_to_this_node: Vec::new(),
        };
        let idx = self.transitions.len();
        self.transition_index_by_id.insert(t.id.clone(), idx);
        self.transitions.push(t);
        self.petri_dirty.push(true);
        Ok(idx)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_timed_transition(
        &mut self,
        id: &str,
        rate: f64,
        priority: i64,
        signal_enabling_expression: &str,
        timer_sec: f64,
        valid_extra_tokens: &[String],
    ) -> Result<usize, PetriNetError> {
        let signal_fn = build_signal_fn(signal_enabling_expression, valid_extra_tokens)?;
        let t = Transition {
            id: id.to_string(),
            rate,
            priority,
            signal_fn,
            is_signal_enabled_val: false,
            is_petri_enabled_val: false,
            is_time_enabled_val: false,
            kind: TransitionKind::Timed {
                timer_sec,
                enabled_since: None,
            },
            arcs_from_this_node: Vec::new(),
            arcs_to_this_node: Vec::new(),
        };
        let idx = self.transitions.len();
        self.transition_index_by_id.insert(t.id.clone(), idx);
        self.transitions.push(t);
        self.petri_dirty.push(true);
        Ok(idx)
    }

    pub fn place_id(&self, id: &str) -> Option<NodeId> {
        self.place_index_by_id.get(id).map(|i| NodeId::Place(*i))
    }

    pub fn transition_id(&self, id: &str) -> Option<NodeId> {
        self.transition_index_by_id
            .get(id)
            .map(|i| NodeId::Transition(*i))
    }

    /// Look up any node by string id (place first, like the Python `or`).
    pub fn node_by_id(&self, id: &str) -> Option<NodeId> {
        self.place_id(id).or_else(|| self.transition_id(id))
    }

    pub fn add_arc(
        &mut self,
        id: &str,
        source: NodeId,
        target: NodeId,
        weight: i64,
        is_inhibitor: bool,
    ) -> Result<usize, PetriNetError> {
        if matches!(
            (source, target),
            (NodeId::Transition(_), NodeId::Transition(_)) | (NodeId::Place(_), NodeId::Place(_))
        ) {
            return Err(PetriNetError::InvalidArc(format!(
                "arc {id} can't connect two nodes of the same type"
            )));
        }

        let arc = Arc {
            id: id.to_string(),
            source,
            target,
            weight,
            is_inhibitor,
        };
        let arc_idx = self.arcs.len();
        self.arcs.push(arc);

        match source {
            NodeId::Place(i) => self.places[i].arcs_from_this_node.push(arc_idx),
            NodeId::Transition(i) => self.transitions[i].arcs_from_this_node.push(arc_idx),
        }
        match target {
            NodeId::Place(i) => self.places[i].arcs_to_this_node.push(arc_idx),
            NodeId::Transition(i) => self.transitions[i].arcs_to_this_node.push(arc_idx),
        }

        // Record adjacency: the transition end depends on the place end's marking.
        let (place_idx, transition_idx) = match (source, target) {
            (NodeId::Place(p), NodeId::Transition(t)) => (p, t),
            (NodeId::Transition(t), NodeId::Place(p)) => (p, t),
            _ => unreachable!("arc validity was checked above"),
        };
        if !self.place_neighbors[place_idx].contains(&transition_idx) {
            self.place_neighbors[place_idx].push(transition_idx);
        }
        Ok(arc_idx)
    }

    fn place_of(&self, node: NodeId) -> &Place {
        match node {
            NodeId::Place(i) => &self.places[i],
            NodeId::Transition(_) => panic!("expected a place node"),
        }
    }

    /// Pure computation of Petri-enabling for a transition (no mutation).
    fn compute_petri_enabled(&self, t: usize) -> bool {
        let transition = &self.transitions[t];
        // Pre-places (arcs into this transition).
        for &arc_idx in &transition.arcs_to_this_node {
            let arc = &self.arcs[arc_idx];
            let preplace = self.place_of(arc.source);
            if (!arc.is_inhibitor && preplace.marking < arc.weight)
                || (arc.is_inhibitor && preplace.marking > 0)
            {
                return false;
            }
        }
        // Post-places (arcs out of this transition).
        for &arc_idx in &transition.arcs_from_this_node {
            let arc = &self.arcs[arc_idx];
            let postplace = self.place_of(arc.target);
            if let Some(cap) = postplace.capacity {
                if arc.weight > cap - postplace.marking {
                    return false;
                }
            }
        }
        true
    }

    pub fn is_petri_enabled(&mut self, t: usize) -> bool {
        let enabled = self.compute_petri_enabled(t);
        self.transitions[t].is_petri_enabled_val = enabled;
        self.petri_dirty[t] = false;
        if !enabled {
            if let TransitionKind::Timed {
                ref mut enabled_since,
                ..
            } = self.transitions[t].kind
            {
                *enabled_since = None;
            }
        }
        enabled
    }

    /// Recompute the enabling arc-scan only if this transition was dirtied by a
    /// marking change since the last computation; otherwise return the cached
    /// value. The result is identical to `is_petri_enabled`, but avoids
    /// re-scanning the arcs of transitions that cannot have changed.
    pub fn ensure_petri_enabled(&mut self, t: usize) -> bool {
        if self.petri_dirty[t] {
            self.is_petri_enabled(t)
        } else {
            self.transitions[t].is_petri_enabled_val
        }
    }

    pub fn is_signal_enabled(&mut self, t: usize, inputs: &HashMap<String, bool>) -> bool {
        let enabled = match &self.transitions[t].signal_fn {
            None => true,
            Some(f) => f(inputs),
        };
        self.transitions[t].is_signal_enabled_val = enabled;
        if !enabled {
            if let TransitionKind::Timed {
                ref mut enabled_since,
                ..
            } = self.transitions[t].kind
            {
                *enabled_since = None;
            }
        }
        enabled
    }

    pub fn is_time_enabled(&mut self, t: usize) -> bool {
        let petri = self.transitions[t].is_petri_enabled_val;
        let signal = self.transitions[t].is_signal_enabled_val;
        if let TransitionKind::Timed {
            timer_sec,
            ref mut enabled_since,
        } = self.transitions[t].kind
        {
            if enabled_since.is_none() && petri && signal {
                *enabled_since = Some(Instant::now());
            }
            let result = match *enabled_since {
                Some(since) => since.elapsed().as_secs_f64() > timer_sec,
                None => false,
            };
            self.transitions[t].is_time_enabled_val = result;
            result
        } else {
            self.transitions[t].is_time_enabled_val = false;
            false
        }
    }

    pub fn reset_timer(&mut self, t: usize) {
        if let TransitionKind::Timed {
            ref mut enabled_since,
            ..
        } = self.transitions[t].kind
        {
            *enabled_since = None;
        }
    }

    /// Fire a transition: consume tokens from pre-places, add to post-places.
    pub fn fire(&mut self, t: usize) -> Result<(), PetriNetError> {
        if !(self.transitions[t].is_signal_enabled_val && self.transitions[t].is_petri_enabled_val)
        {
            // Mirrors the Python RuntimeError; modelled as a logic error.
            panic!("transition cannot fire if it is not enabled");
        }

        let arcs_to = self.transitions[t].arcs_to_this_node.clone();
        let mut changed_places: Vec<usize> = Vec::new();
        for arc_idx in arcs_to {
            let arc = &self.arcs[arc_idx];
            if !arc.is_inhibitor {
                if let NodeId::Place(p) = arc.source {
                    let new_marking = self.places[p].marking - arc.weight;
                    self.places[p].set_marking(new_marking)?;
                    changed_places.push(p);
                }
            }
        }

        let arcs_from = self.transitions[t].arcs_from_this_node.clone();
        for arc_idx in arcs_from {
            let arc = &self.arcs[arc_idx];
            if let NodeId::Place(p) = arc.target {
                let new_marking = self.places[p].marking + arc.weight;
                self.places[p].set_marking(new_marking)?;
                changed_places.push(p);
            }
        }

        // Incremental enabling: only transitions adjacent to a place whose
        // marking changed can have changed their Petri-enabling.
        for p in changed_places {
            for i in 0..self.place_neighbors[p].len() {
                let neighbor = self.place_neighbors[p][i];
                self.petri_dirty[neighbor] = true;
            }
        }

        if let TransitionKind::Timed {
            ref mut enabled_since,
            ..
        } = self.transitions[t].kind
        {
            *enabled_since = None;
        }
        Ok(())
    }
}

/// Internal state of the firing-selection state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InnerState {
    CheckPetriEnabling,
    WaitingFullEnabling,
}

/// A group of transitions bucketed by priority, with a rate accumulator.
#[derive(Default)]
struct TransitionsByPriority {
    max_value_priority: Option<i64>,
    map: BTreeMap<i64, RateList>,
}

#[derive(Default)]
struct RateList {
    transitions: Vec<usize>,
    accumulator: f64,
}

impl TransitionsByPriority {
    fn clear(&mut self) {
        self.max_value_priority = None;
        self.map.clear();
    }

    fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn append(&mut self, transition: usize, priority: i64, rate: f64) {
        self.max_value_priority = Some(match self.max_value_priority {
            Some(m) => m.max(priority),
            None => priority,
        });
        let entry = self.map.entry(priority).or_default();
        entry.transitions.push(transition);
        entry.accumulator += rate;
    }

    /// Iterate the transition indices in this bucket without allocating.
    fn iter_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.map
            .values()
            .flat_map(|l| l.transitions.iter().copied())
    }

    /// Chooses a transition among the highest-priority bucket, weighted by rate.
    fn choose_based_on_priority_and_rate(&self, net: &PetriNet) -> Option<usize> {
        let max_priority = self.max_value_priority?;
        let eligible = self.map.get(&max_priority)?;
        // Fast path: a single eligible transition needs no random draw.
        if eligible.transitions.len() == 1 {
            return Some(eligible.transitions[0]);
        }
        let random_value = rand::random::<f64>() * eligible.accumulator;
        let mut acc = 0.0;
        for &idx in &eligible.transitions {
            acc += net.transitions[idx].rate;
            if random_value <= acc {
                return Some(idx);
            }
        }
        eligible.transitions.last().copied()
    }
}

/// Port of `TransitionsCollection`.
pub struct TransitionsCollection {
    inner_state: InnerState,
    instantaneous: Vec<usize>,
    timed: Vec<usize>,
    petri_enabled_instantaneous: TransitionsByPriority,
    petri_enabled_timed: TransitionsByPriority,
    signal_enabled_instantaneous: TransitionsByPriority,
    signal_enabled_timed: TransitionsByPriority,
    time_enabled_timed: TransitionsByPriority,
}

impl TransitionsCollection {
    pub fn new(net: &PetriNet, transitions: &[usize]) -> Self {
        let mut instantaneous = Vec::new();
        let mut timed = Vec::new();
        for &idx in transitions {
            if net.transitions[idx].is_timed() {
                timed.push(idx);
            } else {
                instantaneous.push(idx);
            }
        }
        TransitionsCollection {
            inner_state: InnerState::CheckPetriEnabling,
            instantaneous,
            timed,
            petri_enabled_instantaneous: TransitionsByPriority::default(),
            petri_enabled_timed: TransitionsByPriority::default(),
            signal_enabled_instantaneous: TransitionsByPriority::default(),
            signal_enabled_timed: TransitionsByPriority::default(),
            time_enabled_timed: TransitionsByPriority::default(),
        }
    }

    /// All transition indices managed by this collection.
    pub fn all_transitions(&self) -> Vec<usize> {
        let mut all = self.instantaneous.clone();
        all.extend(self.timed.iter().copied());
        all
    }

    pub fn reset_timers(&self, net: &mut PetriNet) {
        for &idx in &self.timed {
            net.reset_timer(idx);
        }
    }

    fn update_petri_enabled_instantaneous(&mut self, net: &mut PetriNet) {
        self.petri_enabled_instantaneous.clear();
        for i in 0..self.instantaneous.len() {
            let idx = self.instantaneous[i];
            if net.ensure_petri_enabled(idx) {
                self.petri_enabled_instantaneous.append(
                    idx,
                    net.transitions[idx].priority,
                    net.transitions[idx].rate,
                );
            }
        }
    }

    fn update_petri_enabled_timed(&mut self, net: &mut PetriNet) {
        self.petri_enabled_timed.clear();
        for i in 0..self.timed.len() {
            let idx = self.timed[i];
            if net.ensure_petri_enabled(idx) {
                self.petri_enabled_timed.append(
                    idx,
                    net.transitions[idx].priority,
                    net.transitions[idx].rate,
                );
            }
        }
    }

    fn update_signal_enabled_instantaneous(
        &mut self,
        net: &mut PetriNet,
        inputs: &HashMap<String, bool>,
    ) {
        self.signal_enabled_instantaneous.clear();
        // Disjoint field borrow: iterate the petri-enabled bucket while pushing
        // into the signal-enabled bucket; no intermediate Vec allocation.
        for idx in self.petri_enabled_instantaneous.iter_indices() {
            if net.is_signal_enabled(idx, inputs) {
                self.signal_enabled_instantaneous.append(
                    idx,
                    net.transitions[idx].priority,
                    net.transitions[idx].rate,
                );
            }
        }
    }

    fn update_signal_enabled_timed(&mut self, net: &mut PetriNet, inputs: &HashMap<String, bool>) {
        self.signal_enabled_timed.clear();
        for idx in self.petri_enabled_timed.iter_indices() {
            if net.is_signal_enabled(idx, inputs) {
                self.signal_enabled_timed.append(
                    idx,
                    net.transitions[idx].priority,
                    net.transitions[idx].rate,
                );
            }
        }
    }

    fn update_time_enabled_timed(&mut self, net: &mut PetriNet) {
        self.time_enabled_timed.clear();
        for idx in self.signal_enabled_timed.iter_indices() {
            if net.is_time_enabled(idx) {
                self.time_enabled_timed.append(
                    idx,
                    net.transitions[idx].priority,
                    net.transitions[idx].rate,
                );
            }
        }
    }

    /// Port of `get_transition_chosen_to_fire`.
    pub fn get_transition_chosen_to_fire(
        &mut self,
        net: &mut PetriNet,
        inputs: &HashMap<String, bool>,
        io_has_been_updated: bool,
    ) -> Result<Option<usize>, PetriNetError> {
        if self.inner_state == InnerState::CheckPetriEnabling {
            self.update_petri_enabled_instantaneous(net);
            self.update_petri_enabled_timed(net);
            if self.petri_enabled_instantaneous.is_empty() && self.petri_enabled_timed.is_empty() {
                return Err(PetriNetError::Deadlock);
            }
            self.inner_state = InnerState::WaitingFullEnabling;
            self.update_signal_enabled_instantaneous(net, inputs);
            self.update_signal_enabled_timed(net, inputs);
            self.update_time_enabled_timed(net);
        }

        if self.inner_state == InnerState::WaitingFullEnabling {
            if io_has_been_updated {
                self.update_signal_enabled_instantaneous(net, inputs);
                self.update_signal_enabled_timed(net, inputs);
                self.update_time_enabled_timed(net);
            }

            if !self.signal_enabled_instantaneous.is_empty() {
                self.inner_state = InnerState::CheckPetriEnabling;
                return Ok(self
                    .signal_enabled_instantaneous
                    .choose_based_on_priority_and_rate(net));
            }

            if !self.signal_enabled_timed.is_empty() {
                self.update_time_enabled_timed(net);
                if !self.time_enabled_timed.is_empty() {
                    self.inner_state = InnerState::CheckPetriEnabling;
                    return Ok(self
                        .time_enabled_timed
                        .choose_based_on_priority_and_rate(net));
                }
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    fn empty_inputs() -> HashMap<String, bool> {
        HashMap::new()
    }

    #[test]
    fn test_place_marking_greater_than_capacity_raises() {
        assert!(Place::new("P1", 10, 11).is_err());
        let mut place = Place::new("P2", 7, 3).unwrap();
        assert!(place.set_marking(8).is_err());
    }

    #[test]
    fn test_place_properties() {
        let p = Place::new("P0", 10, 9).unwrap();
        assert_eq!(p.capacity(), Some(10));
        assert_eq!(p.marking(), 9);
        assert_eq!(p.initial_marking(), 9);
    }

    #[test]
    fn test_arc_connecting_two_places_or_two_transitions_raises() {
        let mut net = PetriNet::new();
        let t0 = net
            .add_instantaneous_transition("T0", 1.0, 1, "True", &[])
            .unwrap();
        let t1 = net
            .add_instantaneous_transition("T1", 1.0, 1, "True", &[])
            .unwrap();
        assert!(net
            .add_arc(
                "arc0",
                NodeId::Transition(t0),
                NodeId::Transition(t1),
                1,
                false
            )
            .is_err());
        let p0 = net.add_place(Place::new("P0", 2, 1).unwrap());
        let p1 = net.add_place(Place::new("P1", 2, 1).unwrap());
        assert!(net
            .add_arc("arc1", NodeId::Place(p0), NodeId::Place(p1), 1, false)
            .is_err());
    }

    #[test]
    fn test_transition_enabled_only_if_all_preplaces_have_enough_tokens() {
        let mut net = PetriNet::new();
        let t = net
            .add_instantaneous_transition("T0", 1.0, 1, "True", &[])
            .unwrap();
        for (i, m) in [3, 3, 2].iter().enumerate() {
            let p = net.add_place(Place::new(&format!("P{i}"), 5, *m).unwrap());
            net.add_arc(
                &format!("a{i}"),
                NodeId::Place(p),
                NodeId::Transition(t),
                2,
                false,
            )
            .unwrap();
        }
        assert!(net.is_petri_enabled(t));
        let p = net.add_place(Place::new("P4", 3, 1).unwrap());
        net.add_arc("a4", NodeId::Place(p), NodeId::Transition(t), 2, false)
            .unwrap();
        assert!(!net.is_petri_enabled(t));
    }

    #[test]
    fn test_transition_enabled_only_if_postplaces_have_capacity_left() {
        let mut net = PetriNet::new();
        let t = net
            .add_instantaneous_transition("T0", 1.0, 1, "True", &[])
            .unwrap();
        for (i, (cap, m)) in [(5, 3), (3, 1), (4, 2)].iter().enumerate() {
            let p = net.add_place(Place::new(&format!("P{i}"), *cap, *m).unwrap());
            net.add_arc(
                &format!("a{i}"),
                NodeId::Transition(t),
                NodeId::Place(p),
                2,
                false,
            )
            .unwrap();
        }
        assert!(net.is_petri_enabled(t));
        let p = net.add_place(Place::new("P4", 2, 1).unwrap());
        net.add_arc("a4", NodeId::Transition(t), NodeId::Place(p), 2, false)
            .unwrap();
        assert!(!net.is_petri_enabled(t));
    }

    #[test]
    fn test_firing_decreases_preplaces_and_increases_postplaces() {
        let mut net = PetriNet::new();
        let t = net
            .add_instantaneous_transition("T0", 1.0, 1, "True", &[])
            .unwrap();
        let pre = net.add_place(Place::new("PRE", 5, 3).unwrap());
        let post = net.add_place(Place::new("POST", 5, 1).unwrap());
        net.add_arc("a0", NodeId::Place(pre), NodeId::Transition(t), 2, false)
            .unwrap();
        net.add_arc("a1", NodeId::Transition(t), NodeId::Place(post), 2, false)
            .unwrap();
        assert!(net.is_petri_enabled(t));
        assert!(net.is_signal_enabled(t, &empty_inputs()));
        net.fire(t).unwrap();
        assert_eq!(net.places[pre].marking(), 1);
        assert_eq!(net.places[post].marking(), 3);
    }

    #[test]
    fn test_timed_transition_enabled_only_after_timer() {
        let mut net = PetriNet::new();
        let t = net
            .add_timed_transition("T0", 1.0, 1, "True", 0.03, &[])
            .unwrap();
        assert!(net.is_petri_enabled(t));
        assert!(net.is_signal_enabled(t, &empty_inputs()));
        assert!(!net.is_time_enabled(t));
        sleep(Duration::from_millis(20));
        assert!(!net.is_time_enabled(t));
        sleep(Duration::from_millis(15));
        assert!(net.is_time_enabled(t));
    }

    #[test]
    fn test_timed_transition_timer_resets_after_signal_disabling() {
        let valid = vec!["di0".to_string()];
        let mut net = PetriNet::new();
        let t = net
            .add_timed_transition("T0", 1.0, 1, "di0", 0.03, &valid)
            .unwrap();
        let mut on = HashMap::new();
        on.insert("di0".to_string(), true);
        let mut off = HashMap::new();
        off.insert("di0".to_string(), false);

        assert!(net.is_petri_enabled(t));
        assert!(!net.is_signal_enabled(t, &off));
        assert!(net.is_signal_enabled(t, &on));
        assert!(!net.is_time_enabled(t));
        sleep(Duration::from_millis(35));
        assert!(net.is_time_enabled(t));
        assert!(!net.is_signal_enabled(t, &off));
        assert!(!net.is_time_enabled(t));
    }

    #[test]
    fn test_collection_raises_on_deadlock() {
        let mut net = PetriNet::new();
        let p = net.add_place(Place::new("P0", 1, 0).unwrap());
        let t = net
            .add_instantaneous_transition("T0", 1.0, 1, "True", &[])
            .unwrap();
        net.add_arc("p->t", NodeId::Place(p), NodeId::Transition(t), 1, false)
            .unwrap();
        let mut coll = TransitionsCollection::new(&net, &[t]);
        let res = coll.get_transition_chosen_to_fire(&mut net, &empty_inputs(), false);
        assert_eq!(res, Err(PetriNetError::Deadlock));
    }

    #[test]
    fn test_collection_always_chooses_highest_priority() {
        let mut net = PetriNet::new();
        let p = net.add_place(Place::new("P0", 1, 1).unwrap());
        let t0 = net
            .add_instantaneous_transition("T0", 1.0, 1, "True", &[])
            .unwrap();
        let t1 = net
            .add_instantaneous_transition("T1", 1.0, 2, "True", &[])
            .unwrap();
        let t2 = net
            .add_instantaneous_transition("T2", 1.0, 3, "True", &[])
            .unwrap();
        net.add_arc("a0", NodeId::Place(p), NodeId::Transition(t0), 1, false)
            .unwrap();
        net.add_arc("a1", NodeId::Place(p), NodeId::Transition(t1), 1, false)
            .unwrap();
        net.add_arc("a2", NodeId::Place(p), NodeId::Transition(t2), 1, false)
            .unwrap();
        let mut coll = TransitionsCollection::new(&net, &[t0, t1, t2]);
        for _ in 0..5 {
            let chosen = coll
                .get_transition_chosen_to_fire(&mut net, &empty_inputs(), false)
                .unwrap();
            assert_eq!(chosen, Some(t2));
        }
    }

    #[test]
    fn test_collection_probabilistic_distribution_by_rate() {
        let mut net = PetriNet::new();
        let p = net.add_place(Place::new("P0", 1, 1).unwrap());
        let t0 = net
            .add_instantaneous_transition("T0", 1.0, 1, "True", &[])
            .unwrap();
        let t1 = net
            .add_instantaneous_transition("T1", 2.0, 1, "True", &[])
            .unwrap();
        let t2 = net
            .add_instantaneous_transition("T2", 7.0, 1, "True", &[])
            .unwrap();
        for (i, t) in [t0, t1, t2].iter().enumerate() {
            net.add_arc(
                &format!("a{i}"),
                NodeId::Place(p),
                NodeId::Transition(*t),
                1,
                false,
            )
            .unwrap();
        }
        let mut coll = TransitionsCollection::new(&net, &[t0, t1, t2]);
        let mut counters = [0usize; 3];
        let total = 100_000;
        for _ in 0..total {
            let chosen = coll
                .get_transition_chosen_to_fire(&mut net, &empty_inputs(), false)
                .unwrap()
                .unwrap();
            if chosen == t0 {
                counters[0] += 1;
            } else if chosen == t1 {
                counters[1] += 1;
            } else {
                counters[2] += 1;
            }
        }
        let total_rate = 1.0 + 2.0 + 7.0;
        let expected = [1.0 / total_rate, 2.0 / total_rate, 7.0 / total_rate];
        for i in 0..3 {
            let real = counters[i] as f64 / total as f64;
            assert!(
                (real - expected[i]).abs() < 0.03 * expected[i] + 0.01,
                "rate distribution off for t{i}: real={real}, expected={}",
                expected[i]
            );
        }
    }

    #[test]
    fn test_instantaneous_has_preference_over_timed() {
        let valid = vec!["di0".to_string()];
        let mut net = PetriNet::new();
        let p = net.add_place(Place::new("P0", 1, 1).unwrap());
        let inst = net
            .add_instantaneous_transition("t0", 1.0, 2, "di0", &valid)
            .unwrap();
        let timed = net
            .add_timed_transition("t1", 1.0, 1, "True", 0.03, &[])
            .unwrap();
        net.add_arc("a0", NodeId::Place(p), NodeId::Transition(inst), 1, false)
            .unwrap();
        net.add_arc("a1", NodeId::Place(p), NodeId::Transition(timed), 1, false)
            .unwrap();
        let mut coll = TransitionsCollection::new(&net, &[timed, inst]);

        let mut off = HashMap::new();
        off.insert("di0".to_string(), false);
        let mut on = HashMap::new();
        on.insert("di0".to_string(), true);

        // Prime the timed transition so its timer starts ticking (mirrors the
        // Python test which calls the enabling checks manually first).
        assert!(net.is_petri_enabled(timed));
        assert!(net.is_signal_enabled(timed, &off));
        assert!(!net.is_time_enabled(timed));
        sleep(Duration::from_millis(45));
        assert!(net.is_time_enabled(timed));
        // With di0 off, only the timed transition can fire.
        let chosen = coll
            .get_transition_chosen_to_fire(&mut net, &off, true)
            .unwrap();
        assert_eq!(chosen, Some(timed));
        // With di0 on, the instantaneous transition wins.
        let chosen = coll
            .get_transition_chosen_to_fire(&mut net, &on, true)
            .unwrap();
        assert_eq!(chosen, Some(inst));
    }

    #[test]
    fn test_incremental_enabling_matches_full_recompute() {
        // Two independent 2-cycles + one permanently-idle transition. After each
        // selection (which runs the CHECK phase and ensures every transition),
        // the cached Petri-enabling must equal a fresh full recomputation for
        // *every* transition — proving the incremental cache never diverges.
        let mut net = PetriNet::new();
        let p0 = net.add_place(Place::new("P0", 1, 1).unwrap());
        let p1 = net.add_place(Place::new("P1", 1, 0).unwrap());
        let p2 = net.add_place(Place::new("P2", 1, 1).unwrap());
        let p3 = net.add_place(Place::new("P3", 1, 0).unwrap());
        let pidle = net.add_place(Place::new("PIDLE", 1, 0).unwrap());
        let t0 = net
            .add_instantaneous_transition("T0", 1.0, 1, "True", &[])
            .unwrap();
        let t1 = net
            .add_instantaneous_transition("T1", 1.0, 1, "True", &[])
            .unwrap();
        let t2 = net
            .add_instantaneous_transition("T2", 1.0, 1, "True", &[])
            .unwrap();
        let t3 = net
            .add_instantaneous_transition("T3", 1.0, 1, "True", &[])
            .unwrap();
        let tidle = net
            .add_instantaneous_transition("TIDLE", 1.0, 1, "True", &[])
            .unwrap();
        let p = NodeId::Place;
        let tr = NodeId::Transition;
        net.add_arc("a0", p(p0), tr(t0), 1, false).unwrap();
        net.add_arc("a1", tr(t0), p(p1), 1, false).unwrap();
        net.add_arc("a2", p(p1), tr(t1), 1, false).unwrap();
        net.add_arc("a3", tr(t1), p(p0), 1, false).unwrap();
        net.add_arc("b0", p(p2), tr(t2), 1, false).unwrap();
        net.add_arc("b1", tr(t2), p(p3), 1, false).unwrap();
        net.add_arc("b2", p(p3), tr(t3), 1, false).unwrap();
        net.add_arc("b3", tr(t3), p(p2), 1, false).unwrap();
        net.add_arc("c0", p(pidle), tr(tidle), 1, false).unwrap();

        let mut coll = TransitionsCollection::new(&net, &[t0, t1, t2, t3, tidle]);
        let inputs = HashMap::new();

        for step in 0..300 {
            let chosen = coll
                .get_transition_chosen_to_fire(&mut net, &inputs, false)
                .unwrap();
            // CHECK phase just ran: every cached value must match a full scan.
            for t in 0..net.transitions.len() {
                assert_eq!(
                    net.transitions[t].is_petri_enabled_val,
                    net.compute_petri_enabled(t),
                    "incremental cache diverged at step {step}, transition {t}"
                );
            }
            // TIDLE can never be enabled (its place is empty).
            assert_ne!(chosen, Some(tidle));
            if let Some(t) = chosen {
                net.fire(t).unwrap();
            }
        }
    }
}
