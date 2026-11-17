//! The debugger's agent in `ns-next`: `run --debug` and `--debug-connect`
//! (and `load` alike) speak the Debug Adapter Protocol over a socket on top
//! of the debugging API (`docs/design/debugger.md`, decision 6, "Launching,
//! the start and the end"). Nothing of DAP reaches `ns-runtime`, and
//! nothing a program run without these options does goes through here.
//!
//! `start` attaches the debugger, opens the connection and starts the
//! agent's thread (`agent.rs`), which reads requests off the socket through
//! a reader thread and the VM's events through a pump thread of its own,
//! and answers the host's `HostLink` (`host.rs`) between the host's runs.

mod agent;
mod breakpoints;
mod connection;
mod evaluation;
mod host;
mod protocol;
mod sources;
mod stepping;
mod stopped;
mod wire;

use std::net::TcpStream;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

pub(crate) use host::{HostLink, HostStart, report_terminated, status_of, terminated_before_run};
pub(crate) use sources::{ProgramModule, file_identity};

/// How the agent opens its connection.
pub(crate) enum DebugOptions {
    /// `--debug`: listen on 127.0.0.1 at a port the system chooses.
    Listen,
    /// `--debug-connect <address>`: connect to a client listening there.
    Connect(String),
}

impl DebugOptions {
    /// The options `--debug` and `--debug-connect` ask for, which the
    /// command line keeps exclusive; `None` when neither is given.
    pub(crate) fn from_arguments(debug: bool, connect: Option<String>) -> Option<DebugOptions> {
        match connect {
            Some(address) => Some(DebugOptions::Connect(address)),
            None if debug => Some(DebugOptions::Listen),
            None => None,
        }
    }
}

/// Opens the agent's connection, attaches the debugger to `interpreter`
/// and starts the agent, before the first host run. The listening mode
/// prints its line on the standard error here, before the program writes
/// anything. `program` names the program in the `process` event, and
/// `modules` are its modules as the host loaded them, which a stop shows
/// by their files' paths.
pub(crate) fn start(
    options: &DebugOptions,
    interpreter: &mut ns_runtime::Interpreter,
    program: String,
    modules: Vec<ProgramModule>,
) -> Result<HostLink, ExitCode> {
    let (inputs, received) = mpsc::channel();
    let (mode, connected) = match options {
        DebugOptions::Connect(address) => {
            let stream = TcpStream::connect(address.as_str()).map_err(|error| {
                eprintln!("error: connecting the debugger to {address}: {error}");
                ExitCode::FAILURE
            })?;
            let secret = std::env::var(agent::SECRET_VARIABLE).ok();
            (agent::Mode::Connecting { secret }, Some(stream))
        }
        DebugOptions::Listen => {
            let token = connection::Token::generate().map_err(|error| {
                eprintln!("error: making the debugger's token: {error}");
                ExitCode::FAILURE
            })?;
            let listener = connection::listen()
                .and_then(|listener| Ok((listener.local_addr()?.port(), listener)))
                .map_err(|error| {
                    eprintln!("error: listening for a debugger on 127.0.0.1: {error}");
                    ExitCode::FAILURE
                });
            let (port, listener) = listener?;
            eprintln!(
                "ns-next: debugger listening on 127.0.0.1:{port}, token {}",
                token.hex()
            );
            let (next_connection, next) = mpsc::channel();
            connection::spawn_acceptor(listener, inputs.clone(), next);
            (
                agent::Mode::Listening {
                    token,
                    next_connection,
                },
                None,
            )
        }
    };
    let (control, events) = interpreter.attach_debugger().into_parts();
    let events_detached = Arc::new(AtomicBool::new(false));
    let (drain, drain_requests) = mpsc::channel::<mpsc::Sender<()>>();
    spawn_pump(
        events,
        inputs.clone(),
        Arc::clone(&events_detached),
        drain_requests,
    );
    if let Some(stream) = connected {
        // The receiver lives in the agent, which is started below.
        let _ = inputs.send(agent::Input::Connected(stream));
    }
    let state = Arc::new(host::HostState::new());
    let agent = agent::Agent::new(
        mode,
        inputs.clone(),
        received,
        drain,
        control,
        events_detached,
        Arc::clone(&state),
        program,
        sources::Sources::new(modules),
    );
    std::thread::Builder::new()
        .name("ns-next debugger agent".to_string())
        .spawn(move || agent.run())
        .expect("spawn the debugger's agent");
    Ok(HostLink::new(state, inputs))
}

/// How long the pump waits for an event before it looks for a request of
/// the agent's: short enough that a detach or the program's end is noticed
/// at once, long enough to cost nothing.
const PUMP_SLICE: Duration = Duration::from_millis(10);

/// The pump of the VM's events: hands each to the agent, in order. Ends
/// once the debugger was detached, dropping its half of the remote, or
/// when the agent asks at the program's end, after handing over every
/// event the VM sent until then, which the agent waits for.
fn spawn_pump(
    events: ns_runtime::DebugEvents,
    inputs: mpsc::Sender<agent::Input>,
    detached: Arc<AtomicBool>,
    drain_requests: mpsc::Receiver<mpsc::Sender<()>>,
) {
    std::thread::Builder::new()
        .name("ns-next debugger events".to_string())
        .spawn(move || {
            loop {
                if let Some(event) = events.next_event(PUMP_SLICE)
                    && inputs.send(agent::Input::Event(event)).is_err()
                {
                    return;
                }
                if detached.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(handed_over) = drain_requests.try_recv() {
                    while let Some(event) = events.next_event(Duration::ZERO) {
                        if inputs.send(agent::Input::Event(event)).is_err() {
                            break;
                        }
                    }
                    let _ = handed_over.send(());
                    return;
                }
            }
        })
        .expect("spawn the debugger's event pump");
}
