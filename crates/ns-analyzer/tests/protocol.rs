//! The server end to end over an in-memory connection, as an editor drives it.

#![expect(
    clippy::cast_possible_truncation,
    reason = "a test's values are small literals and lengths it built"
)]

use std::path::{Path, PathBuf};
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
        .spawn(move || ns_analyzer::server::run(server).map_err(|error| error.to_string()))
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
    assert_eq!(capabilities["capabilities"]["foldingRangeProvider"], true);
    assert_eq!(capabilities["capabilities"]["documentSymbolProvider"], true);
    assert_eq!(
        capabilities["capabilities"]["documentFormattingProvider"],
        true
    );
    assert_eq!(capabilities["capabilities"]["codeActionProvider"], true);
    assert_eq!(capabilities["capabilities"]["hoverProvider"], true);
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

    // Over the parameter's use: its declaration, as a `newspeak` block.
    let use_column = fixed.find("^argument").expect("the use") as u64 + 1;
    let hover = request(
        &client,
        4,
        "textDocument/hover",
        json!({ "textDocument": { "uri": URI }, "position": { "line": 0, "character": use_column + 2 } }),
    );
    assert_eq!(hover["contents"]["kind"], "markdown");
    assert_eq!(hover["contents"]["value"], "```newspeak\nargument\n```");
    assert_eq!(
        hover["range"],
        json!({
            "start": { "line": 0, "character": use_column },
            "end": { "line": 0, "character": use_column + "argument".len() as u64 }
        })
    );

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

    let folded = indoc! {"
        class Module = Object new () (
            public run = (
                ^1
            )
        ) : ( make = ( ^self new ) )
    "};
    notify(
        &client,
        "textDocument/didChange",
        json!({ "textDocument": { "uri": URI, "version": 3 }, "contentChanges": [{ "text": folded }] }),
    );
    let _ = next_diagnostics(&client);
    let ranges = request(
        &client,
        5,
        "textDocument/foldingRange",
        json!({ "textDocument": { "uri": URI } }),
    );
    let lines: Vec<(u64, u64)> = ranges
        .as_array()
        .expect("an array of ranges")
        .iter()
        .map(|range| {
            (
                range["startLine"].as_u64().expect("a start line"),
                range["endLine"].as_u64().expect("an end line"),
            )
        })
        .collect();
    assert_eq!(lines, [(0, 4), (1, 3)]);

    let symbols = request(
        &client,
        6,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": URI } }),
    );
    let roots = symbols.as_array().expect("an array of symbols");
    assert_eq!(roots.len(), 1, "one root: the module's class");
    assert_eq!(roots[0]["name"], "Module");
    let children: Vec<(&str, &str)> = roots[0]["children"]
        .as_array()
        .expect("the class's children")
        .iter()
        .map(|child| {
            (
                child["name"].as_str().expect("a name"),
                child["detail"].as_str().expect("a detail"),
            )
        })
        .collect();
    assert_eq!(
        children,
        [
            ("new", "public"),
            ("run", "public"),
            ("make", "protected class")
        ]
    );

    let unopened = request(
        &client,
        7,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": "file:///never/opened.ns" } }),
    );
    assert_eq!(unopened, json!([]), "an unknown document has no symbols");

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

    request(&client, 8, "shutdown", Value::Null);
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
        .spawn(move || ns_analyzer::server::run(server).map_err(|error| error.to_string()))
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
        .spawn(move || ns_analyzer::server::run(server).map_err(|error| error.to_string()))
        .expect("spawn the server");
    request(&client, 1, "initialize", json!({ "capabilities": {} }));
    notify(&client, "initialized", json!({}));
    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(2),
            "textDocument/definition".to_string(),
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

/// A server on its own thread, initialized, and the client end to drive it.
fn initialized_server() -> (Connection, std::thread::JoinHandle<Result<(), String>>) {
    let (server, client) = Connection::memory();
    let server_thread = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || ns_analyzer::server::run(server).map_err(|error| error.to_string()))
        .expect("spawn the server");
    request(&client, 1, "initialize", json!({ "capabilities": {} }));
    notify(&client, "initialized", json!({}));
    (client, server_thread)
}

fn shut_down(client: &Connection, server_thread: std::thread::JoinHandle<Result<(), String>>) {
    request(client, 1000, "shutdown", Value::Null);
    notify(client, "exit", Value::Null);
    let outcome = server_thread
        .join()
        .expect("the server thread does not panic");
    assert!(outcome.is_ok(), "{outcome:?}");
}

/// A directory of its own under the system's temporary directory, empty.
fn scratch_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "ns-analyzer-protocol-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("create the scratch directory");
    directory
}

/// The `file` URI of an absolute path, as an editor writes it: `/`
/// separators, a Windows drive letter after a third slash, and every byte
/// outside the unreserved set percent-encoded -- so a name with a space or
/// a non-ASCII letter reaches the server encoded.
fn file_uri_of(path: &Path) -> String {
    let text = path.to_str().expect("a UTF-8 path").replace('\\', "/");
    let mut uri = String::from("file://");
    if !text.starts_with('/') {
        uri.push('/');
    }
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

fn root_names(symbols: &Value) -> Vec<&str> {
    symbols
        .as_array()
        .expect("an array of symbols")
        .iter()
        .map(|symbol| symbol["name"].as_str().expect("a name"))
        .collect()
}

/// The platform sends `didOpen` only for a file open in an editor; a file
/// selected in the project view and never opened is read from disk.
#[test]
fn a_file_never_opened_answers_its_symbols_from_disk() {
    let directory = scratch_directory("never-opened");
    let path = directory.join("Grüße Module.ns");
    std::fs::write(
        &path,
        indoc! {"
            class Module = Object new () (
                public run = ( ^1 )
            ) : ( make = ( ^self new ) )
        "},
    )
    .expect("write the module");
    let uri = file_uri_of(&path);
    assert!(uri.contains("%20") && uri.contains("%C3%BC"), "{uri}");

    let (client, server_thread) = initialized_server();
    let symbols = request(
        &client,
        2,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    );
    assert_eq!(root_names(&symbols), ["Module"]);
    let children: Vec<&str> = symbols[0]["children"]
        .as_array()
        .expect("the class's children")
        .iter()
        .map(|child| child["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(children, ["new", "run", "make"]);

    // The editor's document has no byte-order mark, so a file saved with
    // one must not shift the positions on its first line.
    let marked = directory.join("Marked.ns");
    std::fs::write(&marked, "\u{feff}class Marked = Object new () ()\n").expect("write the module");
    let symbols = request(
        &client,
        3,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": file_uri_of(&marked) } }),
    );
    assert_eq!(root_names(&symbols), ["Marked"]);
    assert_eq!(
        symbols[0]["selectionRange"]["start"],
        json!({ "line": 0, "character": 6 })
    );
    shut_down(&client, server_thread);
    let _ = std::fs::remove_dir_all(&directory);
}

/// An open document may hold text not yet saved, so it is answered over
/// the file on disk; once closed, the file on disk is answered again.
#[test]
fn an_open_document_wins_over_the_file_on_disk() {
    let directory = scratch_directory("open-wins");
    let path = directory.join("Module.ns");
    std::fs::write(&path, "class OnDisk = Object new () ()\n").expect("write the module");
    let uri = file_uri_of(&path);

    let (client, server_thread) = initialized_server();
    notify(
        &client,
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "newspeak", "version": 1, "text": "class Unsaved = Object new () ()\n" } }),
    );
    let _ = next_diagnostics(&client);
    let symbols = request(
        &client,
        2,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    );
    assert_eq!(root_names(&symbols), ["Unsaved"]);

    notify(
        &client,
        "textDocument/didClose",
        json!({ "textDocument": { "uri": uri } }),
    );
    let _ = next_diagnostics(&client);
    let symbols = request(
        &client,
        3,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    );
    assert_eq!(root_names(&symbols), ["OnDisk"]);
    shut_down(&client, server_thread);
    let _ = std::fs::remove_dir_all(&directory);
}

/// A file that cannot be read, or a URI that names no file, has no
/// symbols, as before: the answer is empty, not an error.
#[test]
fn an_unreadable_file_or_a_uri_that_names_no_file_answers_no_symbols() {
    let directory = scratch_directory("unreadable");
    let (client, server_thread) = initialized_server();
    for (id, uri) in [
        file_uri_of(&directory.join("Missing.ns")),
        // A directory exists but is no file to read.
        file_uri_of(&directory),
        "untitled:Module.ns".to_string(),
        "https://example.org/Module.ns".to_string(),
    ]
    .into_iter()
    .enumerate()
    {
        let symbols = request(
            &client,
            2 + id as i32,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
        );
        assert_eq!(symbols, json!([]), "{uri}");
    }
    shut_down(&client, server_thread);
    let _ = std::fs::remove_dir_all(&directory);
}

/// Opening a FIFO blocks until a writer comes; the server, serving every
/// document on one thread, must answer without opening it.
#[cfg(unix)]
#[test]
fn a_fifo_answers_no_symbols_without_blocking() {
    let directory = scratch_directory("fifo");
    let fifo = directory.join("Module.ns");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("run mkfifo");
    assert!(status.success());
    let (client, server_thread) = initialized_server();
    let symbols = request(
        &client,
        2,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": file_uri_of(&fifo) } }),
    );
    assert_eq!(symbols, json!([]));
    shut_down(&client, server_thread);
    let _ = std::fs::remove_dir_all(&directory);
}

/// Opens `text` as `uri` and waits for the diagnostics the open publishes.
fn open_document(client: &Connection, uri: &str, text: &str) {
    notify(
        client,
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "newspeak", "version": 1, "text": text } }),
    );
    let _ = next_diagnostics(client);
}

fn format_request(client: &Connection, id: i32, uri: &str, options: Value) -> Value {
    request(
        client,
        id,
        "textDocument/formatting",
        json!({ "textDocument": { "uri": uri }, "options": options }),
    )
}

#[test]
fn formatting_answers_one_edit_over_the_whole_document() {
    // Non-ASCII text on the last line, so an end column counted in bytes
    // rather than UTF-16 units would be off.
    let unformatted = "class   Module = Object new () (
public run = ( ^'Grüße 😀' ) )  : ()";
    let formatted = ns_syntax::format(unformatted).expect("the module parses");
    assert_ne!(formatted, unformatted, "the fixture needs formatting");

    let (client, server_thread) = initialized_server();
    open_document(&client, URI, unformatted);
    let edits = format_request(
        &client,
        2,
        URI,
        json!({ "tabSize": 8, "insertSpaces": false }),
    );
    let last_line = unformatted.lines().count() as u64 - 1;
    let last_column = unformatted
        .lines()
        .last()
        .expect("a last line")
        .encode_utf16()
        .count() as u64;
    assert_eq!(
        edits,
        json!([{
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": last_line, "character": last_column },
            },
            "newText": formatted,
        }])
    );
    shut_down(&client, server_thread);
}

#[test]
fn formatting_a_formatted_document_answers_no_edit() {
    let formatted = ns_syntax::format("class Module = Object new () ( public run = ( ^1 ) ) : ()")
        .expect("the module parses");
    let (client, server_thread) = initialized_server();
    open_document(&client, URI, &formatted);
    assert_eq!(
        format_request(
            &client,
            2,
            URI,
            json!({ "tabSize": 4, "insertSpaces": true })
        ),
        json!([])
    );
    shut_down(&client, server_thread);
}

#[test]
fn formatting_a_document_with_a_syntax_error_answers_no_edit() {
    let (client, server_thread) = initialized_server();
    open_document(
        &client,
        URI,
        "class Module = Object new () ( public run = ( ^ ) ",
    );
    assert_eq!(
        format_request(
            &client,
            2,
            URI,
            json!({ "tabSize": 4, "insertSpaces": true })
        ),
        json!([])
    );
    shut_down(&client, server_thread);
}

#[test]
fn formatting_an_unopened_document_answers_no_edit() {
    let (client, server_thread) = initialized_server();
    assert_eq!(
        format_request(
            &client,
            2,
            "file:///never/opened.ns",
            json!({ "tabSize": 4, "insertSpaces": true })
        ),
        json!([])
    );
    shut_down(&client, server_thread);
}

#[test]
fn a_hint_is_published_with_its_quick_fix_and_the_fix_for_the_file() {
    let text = indoc! {"
        class Module = () (
          public run = (
            |
              count
            |
            count:: 1.
            ^count
          )
        )
    "};
    let (client, server_thread) = initialized_server();
    notify(
        &client,
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": URI, "languageId": "newspeak", "version": 1, "text": text } }),
    );
    let published = next_diagnostics(&client);
    let diagnostics = published["diagnostics"].as_array().expect("an array");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["severity"], 4, "a hint");
    assert_eq!(diagnostics[0]["code"], "initialize-in-declaration");
    assert_eq!(diagnostics[0]["source"], "ns-next");
    assert_eq!(
        diagnostics[0]["range"],
        json!({ "start": { "line": 3, "character": 6 }, "end": { "line": 3, "character": 11 } })
    );

    let actions = request(
        &client,
        2,
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": URI },
            "range": { "start": { "line": 3, "character": 7 }, "end": { "line": 3, "character": 7 } },
            "context": { "diagnostics": diagnostics },
        }),
    );
    let actions = actions.as_array().expect("an array of actions");
    let titles: Vec<&str> = actions
        .iter()
        .map(|action| action["title"].as_str().expect("a title"))
        .collect();
    assert_eq!(
        titles,
        [
            "Initialize `count` in its declaration",
            "Apply all hints in this file"
        ]
    );
    assert_eq!(actions[0]["kind"], "quickfix");
    let all = actions[1]["edit"]["changes"][URI]
        .as_array()
        .expect("one edit");
    assert_eq!(all.len(), 1);
    let offset = |position: &Value| -> usize {
        let line = position["line"].as_u64().expect("a line") as usize;
        let character = position["character"].as_u64().expect("a character") as usize;
        text.split_inclusive('\n')
            .take(line)
            .map(str::len)
            .sum::<usize>()
            + character
    };
    let (start, end) = (
        offset(&all[0]["range"]["start"]),
        offset(&all[0]["range"]["end"]),
    );
    let applied = format!(
        "{}{}{}",
        &text[..start],
        all[0]["newText"].as_str().expect("text"),
        &text[end..]
    );
    assert_eq!(applied, ns_analyzer::hints::apply_all(text));
    let edits = actions[0]["edit"]["changes"][URI]
        .as_array()
        .expect("edits for the document");
    assert!(
        edits.iter().any(|edit| edit["newText"] == " ::= 1."),
        "{edits:?}"
    );

    let elsewhere = request(
        &client,
        3,
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": URI },
            "range": { "start": { "line": 6, "character": 4 }, "end": { "line": 6, "character": 4 } },
            "context": { "diagnostics": [] },
        }),
    );
    assert_eq!(elsewhere, json!([]), "no action away from a hint");
    shut_down(&client, server_thread);
}
