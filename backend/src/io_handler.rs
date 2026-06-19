//! IO handlers.
//!
//! Port of `src/implementation/io_handlers.py`.
//!
//! Only `IOWebMocker` (the in-memory emulator) is ported here. It keeps digital
//! inputs/outputs in memory, computes outputs from the place markings via
//! boolean expressions, and tracks whether its inputs changed since the last
//! poll. State lives behind an `Arc<Mutex<..>>` so the web server thread can
//! toggle inputs while the Petri-net thread reads them.
//!
//! TODO(port): `PDR0004_IOHandler` is NOT ported. It drives physical I2C
//! hardware via `smbus` (PCF relays + opto-isolated in/out), which is
//! platform/hardware specific. Porting it would need an I2C crate (e.g.
//! `i2cdev`/`linux-embedded-hal`) behind a cargo feature so non-Pi builds still
//! compile.
//!
//! TODO(port): `IOHandlersWrapper` is NOT ported. It selects between the
//! physical and emulator handlers at runtime. Once a physical handler exists,
//! reintroduce a wrapper implementing `IOHandler` that delegates to the
//! currently-selected backend, and wire the state machine's
//! Physical/EmulatorIOHandlerSelected actions to it (see state_machine.rs).

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::bool_parser::{BoolFn, BoolParser, SyntaxError};

pub use protocol::IoState;

/// Abstract IO handler (port of `AbstractIOHandler`).
pub trait IOHandler: Send + Sync {
    fn set_marking_to_output_expressions(
        &self,
        marking_to_output_expressions: &HashMap<String, String>,
        valid_place_tokens: &[String],
    ) -> Result<(), SyntaxError>;

    /// Recompute outputs from place markings. `places` maps place id -> bool
    /// (`marking > 0`), matching the Python `places_bool`.
    fn update_outputs(&self, places: &HashMap<String, bool>);

    fn clear(&self);

    fn get_all(&self) -> IoState;

    /// True if the digital inputs changed since the previous call.
    fn has_been_updated(&self) -> bool;

    /// Set a single digital input (used by the web mock / IO mocker view).
    fn set_input(&self, id: &str, value: bool);
}

struct Inner {
    output_boolean_functions: Option<HashMap<String, BoolFn>>,
    digital_inputs: BTreeMap<String, bool>,
    previous_digital_inputs: BTreeMap<String, bool>,
    digital_outputs: BTreeMap<String, bool>,
}

/// In-memory IO emulator with optional web control.
#[derive(Clone)]
pub struct IOWebMocker {
    inner: Arc<Mutex<Inner>>,
}

impl IOWebMocker {
    pub fn new(
        digital_inputs: BTreeMap<String, bool>,
        digital_outputs: BTreeMap<String, bool>,
    ) -> Self {
        let previous = digital_inputs.clone();
        IOWebMocker {
            inner: Arc::new(Mutex::new(Inner {
                output_boolean_functions: None,
                digital_inputs,
                previous_digital_inputs: previous,
                digital_outputs,
            })),
        }
    }

    /// Directly set a single input (used by the web mock and by tests).
    pub fn set_input(&self, id: &str, value: bool) {
        let mut inner = self.inner.lock();
        if let Some(slot) = inner.digital_inputs.get_mut(id) {
            *slot = value;
        } else {
            inner.digital_inputs.insert(id.to_string(), value);
        }
    }

    /// Snapshot of just the digital inputs.
    pub fn digital_inputs(&self) -> BTreeMap<String, bool> {
        self.inner.lock().digital_inputs.clone()
    }

    /// Snapshot of just the digital outputs.
    pub fn digital_outputs(&self) -> BTreeMap<String, bool> {
        self.inner.lock().digital_outputs.clone()
    }

    /// Digital inputs as the lowercase-keyed map expected by signal functions.
    pub fn digital_inputs_for_signals(&self) -> HashMap<String, bool> {
        self.inner
            .lock()
            .digital_inputs
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }
}

impl IOHandler for IOWebMocker {
    fn set_marking_to_output_expressions(
        &self,
        marking_to_output_expressions: &HashMap<String, String>,
        valid_place_tokens: &[String],
    ) -> Result<(), SyntaxError> {
        let mut functions = HashMap::new();
        for (output, expression) in marking_to_output_expressions {
            let parser = BoolParser::new(expression, valid_place_tokens)?;
            functions.insert(output.clone(), parser.generate_function());
        }
        self.inner.lock().output_boolean_functions = Some(functions);
        Ok(())
    }

    fn update_outputs(&self, places: &HashMap<String, bool>) {
        let mut inner = self.inner.lock();
        let functions = match inner.output_boolean_functions.take() {
            Some(f) => f,
            None => return,
        };
        for (output, f) in &functions {
            let value = f(places);
            inner.digital_outputs.insert(output.clone(), value);
        }
        inner.output_boolean_functions = Some(functions);
    }

    fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.output_boolean_functions = None;
        for value in inner.digital_outputs.values_mut() {
            *value = false;
        }
    }

    fn get_all(&self) -> IoState {
        let inner = self.inner.lock();
        IoState {
            digital_inputs: inner.digital_inputs.clone(),
            digital_outputs: inner.digital_outputs.clone(),
        }
    }

    fn has_been_updated(&self) -> bool {
        let mut inner = self.inner.lock();
        let changed = inner.digital_inputs != inner.previous_digital_inputs;
        inner.previous_digital_inputs = inner.digital_inputs.clone();
        changed
    }

    fn set_input(&self, id: &str, value: bool) {
        IOWebMocker::set_input(self, id, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mocker() -> IOWebMocker {
        let inputs: BTreeMap<String, bool> = (0..8).map(|i| (format!("i{i}"), false)).collect();
        let outputs: BTreeMap<String, bool> = (0..8).map(|i| (format!("o{i}"), false)).collect();
        IOWebMocker::new(inputs, outputs)
    }

    #[test]
    fn test_has_been_updated_property() {
        let io = mocker();
        assert!(!io.has_been_updated());
        io.set_input("i0", true);
        assert!(io.has_been_updated());
        assert!(!io.has_been_updated());
    }

    #[test]
    fn test_update_outputs_from_place_marking() {
        let io = IOWebMocker::new(BTreeMap::new(), {
            let mut m = BTreeMap::new();
            m.insert("o0".to_string(), false);
            m
        });
        let mut exprs = HashMap::new();
        exprs.insert("o0".to_string(), "P0".to_string());
        io.set_marking_to_output_expressions(&exprs, &["P0".to_string()])
            .unwrap();

        let mut places = HashMap::new();
        // update_outputs expects lowercased place keys (production lowercases
        // once per step); the expression token "P0" parses to "p0".
        places.insert("p0".to_string(), true);
        io.update_outputs(&places);
        assert!(io.get_all().digital_outputs["o0"]);

        places.insert("p0".to_string(), false);
        io.update_outputs(&places);
        assert!(!io.get_all().digital_outputs["o0"]);
    }

    #[test]
    fn test_clear_resets_outputs() {
        let io = mocker();
        io.set_input("i0", true);
        let mut exprs = HashMap::new();
        exprs.insert("o0".to_string(), "true".to_string());
        io.set_marking_to_output_expressions(&exprs, &[]).unwrap();
        io.update_outputs(&HashMap::new());
        assert!(io.get_all().digital_outputs["o0"]);
        io.clear();
        assert!(!io.get_all().digital_outputs["o0"]);
    }
}
