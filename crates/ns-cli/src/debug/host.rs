//! The host's side of the agent: what the thread that runs the program
//! asks the agent and tells it, between its host runs.
//!
//! `ns-next run` is several host runs -- a project's manifest
//! instantiated, the root, then `main:args:` -- and the VM drops a
//! `Terminate` that arrives between two of them (`docs/design/debugger.md`,
//! "Commands while the program runs"), since only the host knows which runs
//! are one program. So the agent records a `terminate` here as well as
//! queuing it, and the host checks the record before each run it starts.

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use super::agent::{HostMessage, Input};

/// What a terminated program reports on its standard error: the line
/// `entry::report_entry_send` prints for the error a terminated run
/// answers, so that a program ended between two runs reports as one ended
/// inside a run does.
pub(crate) const TERMINATED_LINE: &str =
    "Uncaught Newspeak error: the debugger terminated the program";

/// How the host starts once the client configured the session.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostStart {
    /// The program runs; `stop_on_entry` is the `attach`'s, which the agent
    /// acts on when the host announces `main:args:`.
    Run { stop_on_entry: bool },
    /// A `terminate` came first: nothing runs.
    Terminate,
}

/// What the agent and the host share.
pub(crate) struct HostState {
    /// `None` until the client's `configurationDone` -- or a `terminate`,
    /// or the connecting mode's client gone -- decides the start.
    start: Mutex<Option<HostStart>>,
    decided: Condvar,
    terminate: AtomicBool,
}

impl HostState {
    pub(crate) fn new() -> HostState {
        HostState {
            start: Mutex::new(None),
            decided: Condvar::new(),
            terminate: AtomicBool::new(false),
        }
    }

    /// Decides the start, unless it was decided already.
    pub(crate) fn decide(&self, start: HostStart) {
        let mut decided = self.start.lock().unwrap_or_else(PoisonError::into_inner);
        if decided.is_none() {
            *decided = Some(start);
            self.decided.notify_all();
        }
    }

    /// Records a `terminate`: no host run starts from now on, and a host
    /// still waiting for the configuration starts none.
    pub(crate) fn request_terminate(&self) {
        self.terminate.store(true, Ordering::SeqCst);
        self.decide(HostStart::Terminate);
    }

    pub(crate) fn terminate_requested(&self) -> bool {
        self.terminate.load(Ordering::SeqCst)
    }
}

/// The host's end of the agent.
pub(crate) struct HostLink {
    state: Arc<HostState>,
    inputs: Sender<Input>,
}

impl HostLink {
    pub(crate) fn new(state: Arc<HostState>, inputs: Sender<Input>) -> HostLink {
        HostLink { state, inputs }
    }

    /// Waits for the client's `configurationDone`, so that its breakpoints
    /// are set before anything runs.
    pub(crate) fn wait_for_configuration(&self) -> HostStart {
        let mut decided = self
            .state
            .start
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(start) = *decided {
                return start;
            }
            decided = self
                .state
                .decided
                .wait(decided)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Whether a `terminate` arrived: checked before each host run.
    pub(crate) fn terminate_requested(&self) -> bool {
        self.state.terminate_requested()
    }

    /// Tells the agent `main:args:` is about to be sent, and waits until it
    /// has acted on it: with `stopOnEntry` it pauses the remote first, so
    /// that the send's first safepoint stops.
    pub(crate) fn about_to_run_main(&self) {
        self.round_trip(HostMessage::AboutToRunMain);
    }

    /// Tells the agent the program ended with `status`, and waits until it
    /// has told the client, before the process exits.
    pub(crate) fn exited(&self, status: u8) {
        self.round_trip(|done| HostMessage::Exited { status, done });
    }

    fn round_trip(&self, message: impl FnOnce(Sender<()>) -> HostMessage) {
        let (done, acted) = mpsc::channel();
        if self.inputs.send(Input::Host(message(done))).is_ok() {
            // An agent that is gone has nothing left to do for the host.
            let _ = acted.recv();
        }
    }
}

/// Reports a program a `terminate` ended between two host runs.
pub(crate) fn report_terminated() -> ExitCode {
    eprintln!("{TERMINATED_LINE}");
    ExitCode::FAILURE
}

/// Whether the program ends here rather than start its next host run.
pub(crate) fn terminated_before_run(link: Option<&HostLink>) -> bool {
    link.is_some_and(HostLink::terminate_requested)
}

/// The status a command's exit code stands for: every failure the host
/// reports is `ExitCode::FAILURE`.
pub(crate) fn status_of(code: ExitCode) -> u8 {
    if code == ExitCode::SUCCESS {
        0
    } else {
        1
    }
}
