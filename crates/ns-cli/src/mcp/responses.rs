//! The server's standard output: one writer thread owns it, and every
//! thread that answers -- the reader of the standard input, a session's
//! worker, a batch's collector -- hands it whole messages, so that two
//! answers never interleave on the stream the protocol owns and no
//! answer waits for another session's.

use std::collections::{HashSet, VecDeque};
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

/// The most cancellations kept for an answer still to come. A request
/// answered before its cancellation arrived leaves one no answer takes;
/// MCP forbids a client to use an identifier twice, so the oldest such
/// entries are let go rather than kept for the server's life.
const CANCELLATIONS_KEPT: usize = 1024;

/// The way to the writer thread.
#[derive(Clone)]
pub(crate) struct Responses {
    sender: Sender<Value>,
    /// The requests the client cancelled: MCP's cancellation asks for no
    /// response, so whatever answers one later -- the session that served
    /// it, a reply dropped unanswered, a batch's member -- writes nothing,
    /// and this set alone decides it.
    cancelled: Arc<Mutex<Cancellations>>,
}

/// Cancelled requests by their identifiers' JSON text, and the order they
/// were cancelled in, by which the oldest are let go.
#[derive(Default)]
struct Cancellations {
    identifiers: HashSet<String>,
    order: VecDeque<String>,
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
        Ok((Responses::over(sender), writer))
    }

    /// The way to the writer reading `sender`'s channel.
    fn over(sender: Sender<Value>) -> Responses {
        Responses {
            sender,
            cancelled: Arc::new(Mutex::new(Cancellations::default())),
        }
    }

    /// Writes `message` as one line, unless it answers a cancelled
    /// request; answers whether it is written, so that whoever answered
    /// knows whether the client sees what the answer carried.
    pub(crate) fn send(&self, message: Value) -> bool {
        if self.answers_cancelled(&message) {
            return false;
        }
        // The writer ends only once every sender is gone.
        let _ = self.sender.send(message);
        true
    }

    /// Writes a batch's answers, in their order, as one array, and
    /// nothing for none. Each member was decided written as it was handed
    /// to the batch's collector (`deliver`).
    pub(crate) fn send_batch(&self, members: Vec<Value>) {
        if !members.is_empty() {
            let _ = self.sender.send(Value::Array(members));
        }
    }

    /// Notes that the client cancelled its request `identifier`, whose
    /// response is dropped from now on.
    pub(crate) fn cancel(&self, identifier: &Value) {
        let mut cancelled = self.cancelled.lock().expect("the cancelled requests");
        let key = identifier.to_string();
        if cancelled.identifiers.insert(key.clone()) {
            cancelled.order.push_back(key);
        }
        while cancelled.order.len() > CANCELLATIONS_KEPT {
            if let Some(oldest) = cancelled.order.pop_front() {
                cancelled.identifiers.remove(&oldest);
            }
        }
    }

    /// Whether `message` is the response to a cancelled request. A request
    /// is answered once, so its identifier is let go with that answer.
    fn answers_cancelled(&self, message: &Value) -> bool {
        let Some(identifier) = message.get("id") else {
            return false;
        };
        let key = identifier.to_string();
        let mut cancelled = self.cancelled.lock().expect("the cancelled requests");
        if !cancelled.identifiers.remove(&key) {
            return false;
        }
        cancelled.order.retain(|kept| *kept != key);
        true
    }
}

/// Where a reply goes: straight to the standard output, or into the
/// collector of the batch it is a member of, at its member's place.
pub(crate) enum ReplyDestination {
    Responses(Responses),
    /// A batch's member, decided written or dropped by `responses` as it
    /// is handed over: an answer the collector holds is written.
    Batch {
        place: usize,
        collector: Sender<(usize, Value)>,
        responses: Responses,
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
    /// it is an error; answers whether the answer is written, which it is
    /// not for a request the client cancelled.
    pub(crate) fn answer(self, answer: Answer) -> bool {
        let (text, is_error) = match answer {
            Ok(text) => (text, false),
            Err(reason) => (reason, true),
        };
        let result = json!({
            "content": [{ "type": "text", "text": text }],
            "isError": is_error,
        });
        let response = result_response(&self.request_identifier, result);
        self.respond(response)
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

    fn respond(mut self, response: Value) -> bool {
        self.destination
            .take()
            .is_some_and(|destination| deliver(destination, response))
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

/// Hands `response` on to `destination`; answers whether it is written.
fn deliver(destination: ReplyDestination, response: Value) -> bool {
    match destination {
        ReplyDestination::Responses(responses) => responses.send(response),
        ReplyDestination::Batch {
            place,
            collector,
            responses,
        } => {
            if responses.answers_cancelled(&response) {
                return false;
            }
            // A collector outlives every member's sender.
            let _ = collector.send((place, response));
            true
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

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(identifier: i64, destination: ReplyDestination) -> bool {
        Reply::new(json!(identifier), destination).answer(Ok("an answer".to_string()))
    }

    #[test]
    fn a_cancelled_request_s_answer_is_not_written() {
        let (sender, written) = mpsc::channel();
        let responses = Responses::over(sender);
        responses.cancel(&json!(7));
        assert!(!answer(7, ReplyDestination::Responses(responses.clone())));
        assert!(answer(8, ReplyDestination::Responses(responses.clone())));
        let identifiers: Vec<Value> = written
            .try_iter()
            .map(|message| message["id"].clone())
            .collect();
        assert_eq!(identifiers, [json!(8)]);
    }

    #[test]
    fn a_batch_member_cancelled_before_its_hand_over_is_left_out() {
        let (sender, _written) = mpsc::channel();
        let responses = Responses::over(sender);
        let (collector, collected) = mpsc::channel();
        let member = |place: usize| ReplyDestination::Batch {
            place,
            collector: collector.clone(),
            responses: responses.clone(),
        };
        responses.cancel(&json!(1));
        assert!(!answer(1, member(0)));
        assert!(answer(2, member(1)));
        let places: Vec<usize> = collected.try_iter().map(|(place, _)| place).collect();
        assert_eq!(places, [1]);
    }

    #[test]
    fn the_oldest_cancellations_no_answer_took_are_let_go() {
        let (sender, _written) = mpsc::channel();
        let responses = Responses::over(sender);
        let kept = i64::try_from(CANCELLATIONS_KEPT).expect("a small count");
        for identifier in 0..=kept {
            responses.cancel(&json!(identifier));
        }
        assert!(answer(0, ReplyDestination::Responses(responses.clone())));
        assert!(!answer(1, ReplyDestination::Responses(responses.clone())));
        assert!(!answer(kept, ReplyDestination::Responses(responses)));
    }
}
