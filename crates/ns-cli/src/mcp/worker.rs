//! A session's worker: the one thread that owns the session -- its child,
//! its DAP connection, its breakpoints -- and serves its tool calls one
//! after another. Everything that reaches the session arrives on one
//! channel, its inbox: the DAP reader's messages, the calls the reader of
//! the standard input routes to it, and the server's end. A session is
//! one DAP conversation, whose requests and stops answer one another in
//! order, so a call that arrives while another waits is queued and served
//! after it, in arrival order; calls of different sessions never share a
//! thread, so that a wait of one never holds another. This is the DAP
//! agent's own shape (`debug/`), one thread over every input on one
//! channel, applied per session.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::Instant;

use serde_json::Value;

use super::registry::{Registry, SessionHandle};
use super::responses::Reply;
use super::session::SERVER_ENDING;
use super::tools;

/// What reaches a session.
pub(crate) enum Input {
    /// A message off the DAP connection; `None` when it closed.
    Dap(Option<Value>),
    Call(Call),
    /// The client cancelled its request of this JSON-RPC identifier, as
    /// it sent it.
    Cancelled(Value),
    /// The server ends.
    End,
}

/// A tool call routed to a session.
pub(crate) struct Call {
    pub(crate) tool: String,
    pub(crate) arguments: Value,
    pub(crate) reply: Reply,
}

/// The tools that act on a wait in progress rather than wait their turn:
/// the two a debugger's user presses while the program runs.
const INTERRUPTING: [&str; 2] = ["debug_pause", "debug_stop"];

/// A session's inbox, as its worker reads it: the channel, the calls
/// queued while another call waited, whether the server's end came, and
/// the call being served -- its identifier, and whether its client
/// cancelled it.
pub(crate) struct Inbox {
    receiver: Receiver<Input>,
    pending: VecDeque<Call>,
    ended: bool,
    serving: Option<Value>,
    cancelled: bool,
}

impl Inbox {
    pub(crate) fn new(receiver: Receiver<Input>) -> Inbox {
        Inbox {
            receiver,
            pending: VecDeque::new(),
            ended: false,
            serving: None,
            cancelled: false,
        }
    }

    /// The next input, waiting until `deadline`, or for as long as it
    /// takes without one.
    pub(crate) fn receive(&self, deadline: Option<Instant>) -> Result<Input, RecvTimeoutError> {
        match deadline {
            Some(deadline) => self
                .receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now())),
            None => self
                .receiver
                .recv()
                .map_err(|_| RecvTimeoutError::Disconnected),
        }
    }

    /// Keeps `call`, which arrived while another waited, for after it.
    pub(crate) fn queue(&mut self, call: Call) {
        self.pending.push_back(call);
    }

    /// Notes the server's end, which the worker acts on once the call
    /// that waits answered.
    pub(crate) fn end(&mut self) {
        self.ended = true;
    }

    pub(crate) fn ended(&self) -> bool {
        self.ended
    }

    /// The call queued first.
    pub(crate) fn next_pending(&mut self) -> Option<Call> {
        self.pending.pop_front()
    }

    /// Takes the first queued call that acts on a wait in progress -- a
    /// `debug_pause` or a `debug_stop` -- out of its turn.
    pub(crate) fn take_interrupting(&mut self) -> Option<Call> {
        let index = self
            .pending
            .iter()
            .position(|call| INTERRUPTING.contains(&call.tool.as_str()))?;
        self.pending.remove(index)
    }

    /// Moves the first queued `debug_stop` before every other queued
    /// call; answers whether there is one.
    pub(crate) fn stop_first(&mut self) -> bool {
        let Some(index) = self
            .pending
            .iter()
            .position(|call| call.tool == "debug_stop")
        else {
            return false;
        };
        if let Some(stop) = self.pending.remove(index) {
            self.pending.push_front(stop);
        }
        true
    }

    /// Whether `call` acts on a wait in progress.
    pub(crate) fn interrupts(call: &Call) -> bool {
        INTERRUPTING.contains(&call.tool.as_str())
    }

    /// Keeps `call` to be served before every other queued call.
    pub(crate) fn queue_first(&mut self, call: Call) {
        self.pending.push_front(call);
    }

    /// Notes that `call` is served now; `None` between calls.
    pub(crate) fn serve(&mut self, call: Option<&Call>) {
        self.serving = call.map(|call| call.reply.request_identifier().clone());
        self.cancelled = false;
    }

    /// The client cancelled its request `identifier`: the call served, if
    /// it is that one, is to end its wait and answers nothing, and a
    /// queued one of it is dropped -- its answer, which its reply's drop
    /// writes, is one the cancelled set of `Responses` drops.
    pub(crate) fn cancel(&mut self, identifier: &Value) {
        if self.serving.as_ref() == Some(identifier) {
            self.cancelled = true;
        }
        self.pending
            .retain(|call| call.reply.request_identifier() != identifier);
    }

    /// Whether the client cancelled the call served.
    pub(crate) fn cancelled(&self) -> bool {
        self.cancelled
    }

    /// Refuses every call still queued or on its way: the session they
    /// name is gone. The channel is read empty, since the registry routes
    /// nothing more to it once the session is closed; a call routed in
    /// between is answered by its reply's drop.
    fn refuse_remaining(mut self, refusal: impl Fn() -> String) {
        while let Some(call) = self.pending.pop_front() {
            call.reply.answer(Err(refusal()));
        }
        while let Ok(input) = self.receiver.try_recv() {
            if let Input::Call(call) = input {
                call.reply.answer(Err(refusal()));
            }
        }
    }
}

/// Starts the worker of the session `handle`, which reads `receiver` and
/// serves `start`, its `debug_start`, first.
pub(crate) fn spawn(
    registry: Arc<Registry>,
    handle: Arc<SessionHandle>,
    receiver: Receiver<Input>,
    start: Call,
    default_runtime: PathBuf,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(format!("ns-next mcp session {}", handle.number))
        .spawn(move || {
            serve(
                &registry,
                handle,
                Inbox::new(receiver),
                start,
                &default_runtime,
            )
        })
}

fn serve(
    registry: &Registry,
    handle: Arc<SessionHandle>,
    mut inbox: Inbox,
    start: Call,
    default_runtime: &std::path::Path,
) {
    let number = handle.number;
    // However the worker ends -- a panic too -- its session is no longer
    // listed as open; the paths below close it earlier, before they answer.
    let _registration = Registration { registry, number };
    let refusal = || match registry.find(Some(number)) {
        Err(refusal) => refusal,
        Ok(_) => format!("no session {number} is open"),
    };
    inbox.serve(Some(&start));
    let mut session = match tools::start(start.arguments, default_runtime, handle, inbox) {
        Ok((mut session, answer)) => {
            // A cancelled start keeps its session; only its answer goes
            // unseen, and the output it carried is shown by the next.
            let written = start
                .reply
                .answer(Ok(format!("session: {number}\n{answer}")));
            if !written {
                session.show_again_from(0);
            }
            session
        }
        Err((reason, inbox)) => {
            // A start that failed leaves no session; its number is not
            // given again.
            registry.close(number);
            start.reply.answer(Err(reason));
            inbox.refuse_remaining(refusal);
            return;
        }
    };
    while let Some(call) = session.next_call() {
        if call.tool == "debug_stop" {
            let summary = session.stop(tools::STOP_WAIT);
            registry.close(number);
            let inbox = session.take_inbox();
            // What the session owns -- its child above all -- is gone
            // before the answer says so.
            drop(session);
            call.reply
                .answer(Ok(format!("{summary}\nsession {number} is closed")));
            inbox.refuse_remaining(refusal);
            return;
        }
        let shown = session.shown_output();
        let answer = tools::call(&mut session, &call.tool, call.arguments);
        // An answer the client cancelled is not written -- the cancelled
        // set decides it, whether or not the call heard of the
        // cancellation -- so the output it took is shown by the next.
        if !call.reply.answer(answer) {
            session.show_again_from(shown);
        }
    }
    // The server ends: the session's program with it, and what is still
    // queued is answered so.
    registry.close(number);
    let inbox = session.take_inbox();
    drop(session);
    inbox.refuse_remaining(|| SERVER_ENDING.to_string());
}

/// A session's entry in the registry, removed when its worker ends.
struct Registration<'registry> {
    registry: &'registry Registry,
    number: u32,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.registry.close(self.number);
    }
}
