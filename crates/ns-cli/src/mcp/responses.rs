//! The server's standard output: one writer thread owns it, and every
//! thread that answers -- the reader of the standard input, a session's
//! worker, a batch's collector -- hands it whole messages, so that two
//! answers never interleave on the stream the protocol owns and no
//! answer waits for another session's.

use std::collections::HashSet;
use std::io::Write;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde_json::{Value, json};

use super::tools::Answer;

/// The text a tool call is answered with when it was dropped before it
/// was answered: its session's worker ended -- closed, or gone at the
/// server's end -- with the call still queued.
const UNANSWERED: &str = "the session closed before it served the call";

/// The way to the writer thread.
#[derive(Clone)]
pub(crate) struct Responses {
    sender: Sender<Value>,
    /// The requests the client cancelled, by their identifiers' JSON
    /// text: MCP's cancellation asks for no response, so whatever answers
    /// one later -- the session that served it, a reply dropped
    /// unanswered -- writes nothing.
    cancelled: Arc<Mutex<HashSet<String>>>,
}

impl Responses {
    /// Starts the writer thread over the standard output; it ends once
    /// every `Responses` is dropped. A write that fails -- the client
    /// closed its end -- is the last: what follows is dropped, and the
    /// server ends at the end of its input, as it would have otherwise.
    pub(crate) fn start() -> std::io::Result<(Responses, JoinHandle<()>)> {
        let (sender, messages) = mpsc::channel::<Value>();
        let writer = std::thread::Builder::new()
            .name("ns-next mcp writer".to_string())
            .spawn(move || {
                let mut stdout = std::io::stdout();
                let mut broken = false;
                for message in messages {
                    if broken {
                        continue;
                    }
                    let mut text = message.to_string();
                    text.push('\n');
                    broken = stdout
                        .write_all(text.as_bytes())
                        .and_then(|()| stdout.flush())
                        .is_err();
                }
            })?;
        let responses = Responses {
            sender,
            cancelled: Arc::new(Mutex::new(HashSet::new())),
        };
        Ok((responses, writer))
    }

    /// Writes `message` as one line, unless it answers a cancelled request.
    pub(crate) fn send(&self, message: Value) {
        if self.answers_cancelled(&message) {
            return;
        }
        // The writer ends only once every sender is gone.
        let _ = self.sender.send(message);
    }

    /// Writes a batch's answers, in their order, as one array, and
    /// nothing for none. Each member's response passes here as itself,
    /// so that whatever decides whether a response is written decides it
    /// for a batch's member as for a single answer.
    pub(crate) fn send_batch(&self, members: Vec<Value>) {
        let members: Vec<Value> = members
            .into_iter()
            .filter(|member| !self.answers_cancelled(member))
            .collect();
        if !members.is_empty() {
            let _ = self.sender.send(Value::Array(members));
        }
    }

    /// Notes that the client cancelled its request `identifier`, whose
    /// response is dropped from now on.
    pub(crate) fn cancel(&self, identifier: &Value) {
        self.cancelled
            .lock()
            .expect("the cancelled requests")
            .insert(identifier.to_string());
    }

    /// Whether `message` is the response to a cancelled request. A request
    /// is answered once, so its identifier is let go with that answer.
    fn answers_cancelled(&self, message: &Value) -> bool {
        let Some(identifier) = message.get("id") else {
            return false;
        };
        self.cancelled
            .lock()
            .expect("the cancelled requests")
            .remove(&identifier.to_string())
    }
}

/// Where a reply goes: straight to the standard output, or into the
/// collector of the batch it is a member of, at its member's place.
pub(crate) enum ReplyDestination {
    Responses(Responses),
    Batch {
        place: usize,
        collector: Sender<(usize, Value)>,
    },
}

/// The one answer a request is owed. A reply dropped unanswered answers
/// that its session closed first, so that no request of the client's
/// waits for an answer that never comes.
pub(crate) struct Reply {
    request_identifier: Value,
    destination: Option<ReplyDestination>,
}

impl Reply {
    pub(crate) fn new(request_identifier: Value, destination: ReplyDestination) -> Reply {
        Reply {
            request_identifier,
            destination: Some(destination),
        }
    }

    /// Answers a `tools/call`: its text as the one content, and whether
    /// it is an error.
    pub(crate) fn answer(self, answer: Answer) {
        let (text, is_error) = match answer {
            Ok(text) => (text, false),
            Err(reason) => (reason, true),
        };
        let result = json!({
            "content": [{ "type": "text", "text": text }],
            "isError": is_error,
        });
        let response = result_response(&self.request_identifier, result);
        self.respond(response);
    }

    /// The identifier of the request this reply answers.
    pub(crate) fn request_identifier(&self) -> &Value {
        &self.request_identifier
    }

    /// Answers with `result`.
    pub(crate) fn result(self, result: Value) {
        let response = result_response(&self.request_identifier, result);
        self.respond(response);
    }

    /// Answers with the JSON-RPC error `code`.
    pub(crate) fn error(self, code: i64, message: &str) {
        let response = error_response(&self.request_identifier, code, message);
        self.respond(response);
    }

    fn respond(mut self, response: Value) {
        if let Some(destination) = self.destination.take() {
            deliver(destination, response);
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if let Some(destination) = self.destination.take() {
            let result = json!({
                "content": [{ "type": "text", "text": UNANSWERED }],
                "isError": true,
            });
            deliver(
                destination,
                result_response(&self.request_identifier, result),
            );
        }
    }
}

fn deliver(destination: ReplyDestination, response: Value) {
    match destination {
        ReplyDestination::Responses(responses) => responses.send(response),
        ReplyDestination::Batch { place, collector } => {
            // A collector outlives every member's sender.
            let _ = collector.send((place, response));
        }
    }
}

/// Collects the answers of a batch's members, which arrive in any order
/// from the threads that serve them, and writes them as one array in the
/// members' order once every member that is answered has been: the
/// collector ends when the last member's sender is dropped. A batch of
/// notifications only is answered with nothing.
pub(crate) fn collect_batch(responses: Responses) -> std::io::Result<Sender<(usize, Value)>> {
    let (collector, answers) = mpsc::channel::<(usize, Value)>();
    std::thread::Builder::new()
        .name("ns-next mcp batch".to_string())
        .spawn(move || {
            let mut collected: Vec<(usize, Value)> = answers.iter().collect();
            collected.sort_by_key(|(place, _)| *place);
            responses.send_batch(collected.into_iter().map(|(_, answer)| answer).collect());
        })?;
    Ok(collector)
}

pub(crate) fn result_response(identifier: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": identifier, "result": result })
}

pub(crate) fn error_response(identifier: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": identifier, "error": { "code": code, "message": message } })
}
