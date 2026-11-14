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
mod connection;
mod host;
mod protocol;
mod wire;

use std::net::TcpStream;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

pub(crate) use host::{report_terminated, status_of, terminated_before_run, HostLink, HostStart};

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
/// anything. `program` names the program in the `process` event.
pub(crate) fn start(
    options: &DebugOptions,
    interpreter: &mut ns_runtime::Interpreter,
    program: String,
) -> Result<HostLink, ExitCode> {
    let (inputs, received) = mpsc::channel();
    let (mode, connected) = match options {
        DebugOptions::Connect(address) => {
            let stream = TcpStream::connect(address.as_str()).map_err(|error| {
                eprintln!("error: connecting the debugger to {address}: {error}");
                ExitCode::FAILURE
            })?;
            (agent::Mode::Connecting, Some(stream))
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
    let pump_detached = Arc::clone(&events_detached);
    let pump_inputs = inputs.clone();
    std::thread::Builder::new()
        .name("ns-next debugger events".to_string())
        .spawn(move || {
            // Ends with the VM, or at the first event after the debugger
            // was detached, dropping this half of the remote.
            while let Some(event) = events.recv() {
                if pump_detached.load(Ordering::SeqCst)
                    || pump_inputs.send(agent::Input::Event(event)).is_err()
                {
                    break;
                }
            }
        })
        .expect("spawn the debugger's event pump");
    if let Some(stream) = connected {
        // The receiver lives in the agent, which is started below.
        let _ = inputs.send(agent::Input::Connected(stream));
    }
    let state = Arc::new(host::HostState::new());
    let agent = agent::Agent::new(
        mode,
        inputs.clone(),
        control,
        events_detached,
        Arc::clone(&state),
        program,
    );
    std::thread::Builder::new()
        .name("ns-next debugger agent".to_string())
        .spawn(move || agent.run(received))
        .expect("spawn the debugger's agent");
    Ok(HostLink::new(state, inputs))
}
