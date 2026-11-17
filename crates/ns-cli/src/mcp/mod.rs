//! `ns-next mcp`: a Model Context Protocol server over standard input and
//! output, through which an agent such as Claude debugs a Newspeak
//! program (`docs/design/debugger.md`, "The MCP server"). JSON-RPC 2.0,
//! one message a line, the specification's revision 2025-06-18:
//! `initialize`, `ping`, `tools/list` and `tools/call`, written by hand on
//! `serde_json` as the DAP agent's framing is, so that no protocol crate
//! reaches `ns-next`. Nothing but protocol messages is written to the
//! standard output; what the server has to say otherwise goes to the
//! standard error. The program runs in a child the session starts, a DAP
//! client of whose agent the server is (`session.rs`, `dap_client.rs`).

mod dap_client;
mod session;
mod shipped_sources;
mod summary;
mod tools;

use std::io::{BufRead, Write};
use std::process::ExitCode;

use serde_json::{Value, json};

/// The revisions of the specification the server speaks, the latest
/// first, which it answers when a client asks for one it does not know.
/// The tools are the same in each.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// JSON-RPC's error codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// `ns-next mcp`: removes the stale shipped sources of other binaries
/// (`shipped_sources.rs`), then serves until the client closes the
/// standard input, one message at a time -- a tool call holds the loop for
/// as long as it waits. A `debug_start` runs the program with this very
/// executable unless told otherwise. The session's program ends with the
/// server.
pub(crate) fn execute() -> ExitCode {
    shipped_sources::remove_stale_core_libraries();
    let default_runtime = std::env::current_exe().unwrap_or_else(|error| {
        eprintln!("ns-next mcp: the executable's path is unknown: {error}");
        std::path::PathBuf::from("ns-next")
    });
    let mut debugger = tools::Debugger::new(default_runtime);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(answer) = answer(&mut debugger, &line) {
            let mut text = answer.to_string();
            text.push('\n');
            if stdout
                .write_all(text.as_bytes())
                .and_then(|()| stdout.flush())
                .is_err()
            {
                break;
            }
        }
    }
    ExitCode::SUCCESS
}

/// The answer to one line the client sent: a response, or nothing for a
/// notification or a response of the client's; a batch, which JSON-RPC
/// and the revisions before 2025-06-18 allow, is answered as one.
fn answer(debugger: &mut tools::Debugger, line: &str) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(error) => {
            return Some(error_response(
                &Value::Null,
                PARSE_ERROR,
                &format!("not JSON: {error}"),
            ));
        }
    };
    if let Value::Array(messages) = message {
        let answers: Vec<Value> = messages
            .into_iter()
            .filter_map(|message| answer_message(debugger, &message))
            .collect();
        return (!answers.is_empty()).then_some(Value::Array(answers));
    }
    answer_message(debugger, &message)
}

/// The answer to one message.
fn answer_message(debugger: &mut tools::Debugger, message: &Value) -> Option<Value> {
    let Some(method) = message["method"].as_str() else {
        // A response to a request of the server's, which sends none, or
        // no message at all.
        return message
            .get("result")
            .or_else(|| message.get("error"))
            .is_none()
            .then(|| error_response(&message["id"], INVALID_REQUEST, "no method"));
    };
    // A notification -- `notifications/initialized`,
    // `notifications/cancelled` -- has no id and asks for no answer.
    let id = message.get("id")?;
    let params = &message["params"];
    Some(match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str();
            let version = asked
                .filter(|asked| PROTOCOL_VERSIONS.contains(asked))
                .unwrap_or(PROTOCOL_VERSIONS[0]);
            result_response(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "ns-next", "version": env!("CARGO_PKG_VERSION") },
                }),
            )
        }
        "ping" => result_response(id, json!({})),
        "tools/list" => result_response(id, tools::list()),
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return Some(error_response(
                    id,
                    INVALID_PARAMS,
                    "a tool call names its tool",
                ));
            };
            let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
            match debugger.call(name, arguments) {
                Some(answer) => {
                    let (text, is_error) = match answer {
                        Ok(text) => (text, false),
                        Err(reason) => (reason, true),
                    };
                    result_response(
                        id,
                        json!({
                            "content": [{ "type": "text", "text": text }],
                            "isError": is_error,
                        }),
                    )
                }
                None => error_response(id, INVALID_PARAMS, &format!("no tool is named {name}")),
            }
        }
        _ => error_response(id, METHOD_NOT_FOUND, &format!("no method {method}")),
    })
}

fn result_response(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}
