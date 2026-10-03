//! The server end to end over an in-memory connection, as an editor drives it.

use std::time::Duration;

use indoc::indoc;
use lsp_server::{Connection, Message, Notification, Request, RequestId};
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(10);

fn receive(client: &Connection) -> Message {
    client
        .receiver
        .recv_timeout(TIMEOUT)
        .expect("the server answers within the timeout")
}

fn request(client: &Connection, id: i32, method: &str, params: Value) -> Value {
    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(id),
            method.to_string(),
            params,
        )))
        .expect("send a request");
    loop {
        if let Message::Response(response) = receive(client) {
            assert_eq!(response.id, RequestId::from(id));
            // `lsp_server::Response` carries its outcome as one
            // `response_result: Result<Value, ResponseError>` field -- there
            // is no separate `result`/`error` pair to read.
            return match response.response_result {
                Ok(result) => result,
                Err(error) => panic!("{method}: {error:?}"),
            };
        }
    }
}

fn notify(client: &Connection, method: &str, params: Value) {
    client
        .sender
        .send(Message::Notification(Notification::new(
            method.to_string(),
            params,
        )))
        .expect("send a notification");
}

fn next_diagnostics(client: &Connection) -> Value {
    loop {
        if let Message::Notification(notification) = receive(client) {
            if notification.method == "textDocument/publishDiagnostics" {
                return notification.params;
            }
        }
    }
}

const URI: &str = "file:///C:/work/Module.ns";

#[test]
fn an_editor_session() {
    let (server, client) = Connection::memory();
    let server_thread = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || ns_lsp::server::run(server).map_err(|error| error.to_string()))
        .expect("spawn the server");

    let capabilities = request(&client, 1, "initialize", json!({ "capabilities": {} }));
    assert_eq!(
        capabilities["capabilities"]["textDocumentSync"], 1,
        "full sync"
    );
    assert_eq!(
        capabilities["capabilities"]["semanticTokensProvider"]["legend"]["tokenTypes"],
        json!([
            "parameter",
            "variable",
            "property",
            "class",
            "method",
            "keyword"
        ])
    );
    notify(&client, "initialized", json!({}));

    // Non-ASCII text before the error on its line, so a position counted in
    // bytes or in chars rather than UTF-16 units would be off.
    let broken = indoc! {"
        class Module = Object new () ( (* Grüße 😀 *) public broken = ( ^outer Missing foo ) ) : ()
    "};
    notify(
        &client,
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": URI, "languageId": "newspeak", "version": 1, "text": broken } }),
    );
    let published = next_diagnostics(&client);
    assert_eq!(published["uri"], URI);
    let diagnostics = published["diagnostics"].as_array().expect("an array");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["severity"], 1, "an error");
    assert_eq!(diagnostics[0]["source"], "ns-next");
    let line = broken.lines().next().expect("one line");
    let byte_column = line.find("outer Missing").expect("the send");
    let column = line[..byte_column].encode_utf16().count() as u64;
    assert_eq!(
        diagnostics[0]["range"]["start"],
        json!({ "line": 0, "character": column })
    );

    let fixed = indoc! {"
        class Module = Object new () ( public echo: argument = ( ^argument ) ) : ()
    "};
    notify(
        &client,
        "textDocument/didChange",
        json!({ "textDocument": { "uri": URI, "version": 2 }, "contentChanges": [{ "text": fixed }] }),
    );
    let published = next_diagnostics(&client);
    assert_eq!(published["diagnostics"], json!([]));

    let tokens = request(
        &client,
        2,
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": URI } }),
    );
    let data = tokens["data"].as_array().expect("token data");
    assert!(!data.is_empty(), "the fixed module has classified names");
    assert_eq!(data.len() % 5, 0, "five integers per token");

    let unknown = request(
        &client,
        3,
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": "file:///never/opened.ns" } }),
    );
    assert_eq!(
        unknown["data"],
        json!([]),
        "an unknown document has no tokens"
    );

    notify(
        &client,
        "textDocument/didClose",
        json!({ "textDocument": { "uri": URI } }),
    );
    let published = next_diagnostics(&client);
    assert_eq!(
        published["diagnostics"],
        json!([]),
        "closing clears the errors"
    );

    request(&client, 4, "shutdown", Value::Null);
    notify(&client, "exit", Value::Null);
    let outcome = server_thread
        .join()
        .expect("the server thread does not panic");
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[test]
fn a_malformed_notification_is_reported_and_the_server_keeps_serving() {
    let (server, client) = Connection::memory();
    let server_thread = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || ns_lsp::server::run(server).map_err(|error| error.to_string()))
        .expect("spawn the server");
    request(&client, 1, "initialize", json!({ "capabilities": {} }));
    notify(&client, "initialized", json!({}));

    // `textDocument` must be an object with `uri`/`languageId`/`version`/
    // `text`; a bare string fails `DidOpenTextDocumentParams` deserialization.
    notify(
        &client,
        "textDocument/didOpen",
        json!({ "textDocument": "not an object" }),
    );

    // A notification has no response; the client hears of it as an error in
    // its log for the server.
    let Message::Notification(report) = receive(&client) else {
        panic!("expected a notification");
    };
    assert_eq!(report.method, "window/logMessage");
    assert_eq!(report.params["type"], json!(1), "MessageType::ERROR");
    let message = report.params["message"].as_str().expect("a message");
    assert!(
        message.starts_with("malformed textDocument/didOpen notification: "),
        "{message}"
    );

    // The server must still be alive and answering: a request right after
    // the malformed notification gets a real response, not a timeout.
    let tokens = request(
        &client,
        2,
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": "file:///never/opened.ns" } }),
    );
    assert_eq!(tokens["data"], json!([]));

    request(&client, 3, "shutdown", Value::Null);
    notify(&client, "exit", Value::Null);
    let outcome = server_thread
        .join()
        .expect("the server thread does not panic");
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[test]
fn an_unknown_request_is_method_not_found() {
    let (server, client) = Connection::memory();
    let server_thread = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || ns_lsp::server::run(server).map_err(|error| error.to_string()))
        .expect("spawn the server");
    request(&client, 1, "initialize", json!({ "capabilities": {} }));
    notify(&client, "initialized", json!({}));
    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(2),
            "textDocument/hover".to_string(),
            json!({}),
        )))
        .expect("send");
    let Message::Response(response) = receive(&client) else {
        panic!("expected a response");
    };
    assert_eq!(
        response.response_result.err().map(|error| error.code),
        Some(lsp_server::ErrorCode::MethodNotFound as i32)
    );
    request(&client, 3, "shutdown", Value::Null);
    notify(&client, "exit", Value::Null);
    assert!(server_thread.join().expect("no panic").is_ok());
}
