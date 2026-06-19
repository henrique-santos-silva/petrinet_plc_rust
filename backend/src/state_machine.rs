//! State machine.
//!
//! Port of `src/implementation/state_machine.py`.
//!
//! Drives the whole system: reacts to web-UI events and Petri-net events,
//! transitions between states, executes the matching actions (setup/start/
//! pause/resume/finish, IO module selection) and keeps the web UI in sync.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use crate::io_handler::IOHandler;
use crate::petri_net_handler::{Event as PetriNetEvent, PetriNetHandler};
use crate::webserver_handler::{WebServerEvent, WebServerHandler};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Init,
    CheckingPetriNetFilesExistence,
    WaitingPetriNetFilesUpload,
    PetriNetFilesUploaded,
    Running,
    DeadLock,
    Paused,
    WaitingEndOfCycle,
}

impl State {
    /// Matches the Python `States.name` used by the frontend.
    pub fn name(&self) -> &'static str {
        match self {
            State::Init => "INIT",
            State::CheckingPetriNetFilesExistence => "CheckingPetriNetFilesExistence",
            State::WaitingPetriNetFilesUpload => "WaitingPetriNetFilesUpload",
            State::PetriNetFilesUploaded => "PetriNetFilesUploaded",
            State::Running => "Running",
            State::DeadLock => "DeadLock",
            State::Paused => "Paused",
            State::WaitingEndOfCycle => "WaitingEndOfCycle",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InternalEvent {
    None,
    Init,
    FilesAlreadyUploaded,
    FilesNotUploadedYet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    DoNothing,
    CheckPetriNetFilesExistence,
    StartExecution,
    PauseExecution,
    ResumeExecution,
    FinishExecution,
    PhysicalIOHandlerSelected,
    EmulatorIOHandlerSelected,
}

/// Unified event type the transition table matches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    Internal(InternalEvent),
    WebServer(WebServerEvent),
    PetriNet(PetriNetEvent),
}

pub struct StateMachine {
    petrinet_handler: PetriNetHandler,
    webserver_handler: WebServerHandler,
    io: Arc<dyn IOHandler>,
    /// TODO(port): always `false` because the physical IO handler
    /// (`PDR0004_IOHandler`) and `IOHandlersWrapper` are not ported yet (see
    /// io_handler.rs). When they are, this should reflect real hardware
    /// availability and the Physical/Emulator actions below should switch the
    /// active backend instead of being effectively no-ops.
    physical_io_enabled: bool,

    state: State,
    internal_event: InternalEvent,

    webserver_events: Receiver<WebServerEvent>,
    petrinet_events: Receiver<PetriNetEvent>,
}

impl StateMachine {
    pub fn new(
        petrinet_handler: PetriNetHandler,
        webserver_handler: WebServerHandler,
        io: Arc<dyn IOHandler>,
        webserver_events: Receiver<WebServerEvent>,
        petrinet_events: Receiver<PetriNetEvent>,
    ) -> Self {
        StateMachine {
            petrinet_handler,
            webserver_handler,
            io,
            physical_io_enabled: false,
            state: State::Init,
            internal_event: InternalEvent::Init,
            webserver_events,
            petrinet_events,
        }
    }

    /// Port of `_get_event`: internal events take priority, then web, then net.
    fn get_event(&mut self) -> Trigger {
        if self.internal_event != InternalEvent::None {
            return Trigger::Internal(self.internal_event);
        }
        match self.webserver_events.try_recv() {
            Ok(event) => return Trigger::WebServer(event),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {}
        }
        match self.petrinet_events.try_recv() {
            Ok(event) => return Trigger::PetriNet(event),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {}
        }
        Trigger::Internal(InternalEvent::None)
    }

    /// Port of the `_state_machine_dictionary` lookup. Returns the next state
    /// and the action to execute, or `None` to stay put doing nothing.
    fn transition(&self, trigger: Trigger) -> (State, Action) {
        use Action as A;
        use State as S;
        use WebServerEvent as W;

        let stay = (self.state, A::DoNothing);

        match (self.state, trigger) {
            (S::Init, Trigger::Internal(InternalEvent::Init)) => (
                S::CheckingPetriNetFilesExistence,
                A::CheckPetriNetFilesExistence,
            ),
            (
                S::CheckingPetriNetFilesExistence,
                Trigger::Internal(InternalEvent::FilesNotUploadedYet),
            ) => (S::WaitingPetriNetFilesUpload, A::DoNothing),
            (
                S::CheckingPetriNetFilesExistence,
                Trigger::Internal(InternalEvent::FilesAlreadyUploaded),
            ) => (S::PetriNetFilesUploaded, A::DoNothing),
            (S::WaitingPetriNetFilesUpload, Trigger::WebServer(W::PetriNetFilesUploaded)) => {
                (S::PetriNetFilesUploaded, A::DoNothing)
            }
            (S::PetriNetFilesUploaded, Trigger::WebServer(W::StartExecution)) => {
                (S::Running, A::StartExecution)
            }
            (S::PetriNetFilesUploaded, Trigger::WebServer(W::PhysicalIOHandlerSelected)) => {
                (S::PetriNetFilesUploaded, A::PhysicalIOHandlerSelected)
            }
            (S::PetriNetFilesUploaded, Trigger::WebServer(W::EmulatorIOHandlerSelected)) => {
                (S::PetriNetFilesUploaded, A::EmulatorIOHandlerSelected)
            }
            (S::Running, Trigger::PetriNet(PetriNetEvent::Deadlock)) => (S::DeadLock, A::DoNothing),
            (S::Running, Trigger::WebServer(W::PauseExecution)) => (S::Paused, A::PauseExecution),
            (S::Running, Trigger::WebServer(W::FinishExecutionAfterCycle)) => {
                (S::WaitingEndOfCycle, A::DoNothing)
            }
            (S::Running, Trigger::WebServer(W::FinishExecutionImmediately)) => {
                (S::PetriNetFilesUploaded, A::FinishExecution)
            }
            (S::DeadLock, Trigger::WebServer(W::FinishExecutionImmediately)) => {
                (S::PetriNetFilesUploaded, A::FinishExecution)
            }
            (S::Paused, Trigger::WebServer(W::ResumeExecution)) => (S::Running, A::ResumeExecution),
            (S::Paused, Trigger::WebServer(W::FinishExecutionImmediately)) => {
                (S::PetriNetFilesUploaded, A::FinishExecution)
            }
            (S::WaitingEndOfCycle, Trigger::PetriNet(PetriNetEvent::CycleFinished)) => {
                (S::PetriNetFilesUploaded, A::FinishExecution)
            }
            (S::WaitingEndOfCycle, Trigger::WebServer(W::FinishExecutionImmediately)) => {
                (S::PetriNetFilesUploaded, A::FinishExecution)
            }
            _ => stay,
        }
    }

    fn exec_action(&mut self, action: Action) {
        self.internal_event = InternalEvent::None;
        match action {
            Action::DoNothing => {}
            Action::CheckPetriNetFilesExistence => {
                self.webserver_handler.post_current_io_module(
                    Some(self.physical_io_enabled),
                    Some(self.physical_io_enabled),
                );
                self.internal_event = if self.webserver_handler.check_petri_net_files_existence() {
                    InternalEvent::FilesAlreadyUploaded
                } else {
                    InternalEvent::FilesNotUploadedYet
                };
            }
            Action::StartExecution => {
                if let Ok(iopt) = self.webserver_handler.get_file() {
                    if let Err(e) = self.petrinet_handler.setup(&iopt) {
                        eprintln!("setup error: {e}");
                    } else {
                        self.petrinet_handler.set_running_flag(true);
                    }
                }
            }
            Action::PauseExecution => {
                self.petrinet_handler.set_running_flag(false);
                self.petrinet_handler.reset_timers();
            }
            Action::ResumeExecution => {
                self.petrinet_handler.set_running_flag(true);
            }
            Action::FinishExecution => {
                self.petrinet_handler.set_running_flag(false);
                self.io.clear();
            }
            Action::PhysicalIOHandlerSelected => {
                // TODO(port): no real switch happens — the physical handler and
                // IOHandlersWrapper are not ported (see io_handler.rs). For now
                // we only echo the (disabled) module state back to the UI.
                self.webserver_handler.post_current_io_module(
                    Some(self.physical_io_enabled),
                    Some(self.physical_io_enabled),
                );
            }
            Action::EmulatorIOHandlerSelected => {
                // TODO(port): once IOHandlersWrapper exists, actually select the
                // emulator backend here instead of only updating the UI.
                self.webserver_handler
                    .post_current_io_module(Some(false), Some(self.physical_io_enabled));
            }
        }
    }

    /// Port of `run`: the main loop.
    pub fn run(mut self) {
        loop {
            sleep(Duration::from_millis(50));
            let trigger = self.get_event();
            let (next_state, action) = self.transition(trigger);
            self.state = next_state;
            self.exec_action(action);
            self.webserver_handler.post_state(self.state.name());
            self.webserver_handler.post_ios(self.io.get_all());
        }
    }

    /// Execute a single iteration of the loop (used by tests).
    pub fn run_once(&mut self) {
        let trigger = self.get_event();
        let (next_state, action) = self.transition(trigger);
        self.state = next_state;
        self.exec_action(action);
        self.webserver_handler.post_state(self.state.name());
        self.webserver_handler.post_ios(self.io.get_all());
    }

    pub fn state(&self) -> State {
        self.state
    }
}
