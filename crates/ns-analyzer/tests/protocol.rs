//! The server end to end over an in-memory connection, as an editor drives it.

#![expect(
    clippy::cast_possible_truncation,
    reason = "a test's values are small literals and lengths it built"
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use indoc::indoc;
use lsp_server::{Connection, Message, Notification, Request, RequestId};
use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(10);

/// The timeout of every answer a test that compiles a module waits for,
/// from `initialize` to `shutdown`. Such a test bootstraps the worker's
/// interpreter, so it reaches the runtime and the stress gate runs it
/// (`xtask/unreached-tests.txt` does not list it), alongside its fifteen
/// other suites, where a test of 0.03 s took 9 to 34 s and one of 0.2 s
/// more than 25 (and `disasm_arithmetic_initializer` 19.5 s for 0.03 s):
/// merely slow, as nothing of theirs grows under a stress mode. The
/// stall is the whole process's, not the compile's: three of these
/// tests, whose answers take about 2 ms together on an idle machine,
/// each ran 18 s there and timed out at once waiting 10 s for an
/// `initialize` or a `shutdown`, which the server answers on its own
/// thread without asking the worker. So every wait of such a test takes
/// this timeout, not only those behind a compile. Long enough not to fail
/// a loaded machine, short enough that a server that never answers still
/// fails the test.
const STRESS_TIMEOUT: Duration = Duration::from_secs(300);

// The waiting helpers track their caller, so that a timeout names the
// test's line that waited rather than the helper's.

#[track_caller]
fn receive(client: &Connection) -> Message {
    receive_within(client, TIMEOUT)
}

#[track_caller]
fn receive_within(client: &Connection, timeout: Duration) -> Message {
    client
        .receiver
        .recv_timeout(timeout)
        .expect("the server answers within the timeout")
}

#[track_caller]
fn request(client: &Connection, id: i32, method: &str, params: Value) -> Value {
    request_within(client, id, method, params, TIMEOUT)
}

#[track_caller]
fn request_within(
    client: &Connection,
    id: i32,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Value {
    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(id),
            method.to_string(),
            params,
        )))
        .expect("send a request");
    loop {
        if let Message::Response(response) = receive_within(client, timeout) {
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

#[track_caller]
fn next_diagnostics(client: &Connection) -> Value {
    next_diagnostics_within(client, TIMEOUT)
}

#[track_caller]
fn next_diagnostics_within(client: &Connection, timeout: Duration) -> Value {
    loop {
        if let Message::Notification(notification) = receive_within(client, timeout)
            && notification.method == "textDocument/publishDiagnostics"
        {
            return notification.params;
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
#[track_caller]
fn initialized_server() -> (Connection, std::thread::JoinHandle<Result<(), String>>) {
    initialized_server_within(TIMEOUT)
}

/// `initialized_server`, waiting up to `timeout` for the server's answer.
#[track_caller]
fn initialized_server_within(
    timeout: Duration,
) -> (Connection, std::thread::JoinHandle<Result<(), String>>) {
    let (server, client) = Connection::memory();
    let server_thread = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || ns_analyzer::server::run(server).map_err(|error| error.to_string()))
        .expect("spawn the server");
    request_within(
        &client,
        1,
        "initialize",
        json!({ "capabilities": {} }),
        timeout,
    );
    notify(&client, "initialized", json!({}));
    (client, server_thread)
}

#[track_caller]
fn shut_down(client: &Connection, server_thread: std::thread::JoinHandle<Result<(), String>>) {
    shut_down_within(client, server_thread, TIMEOUT);
}

/// `shut_down`, waiting up to `timeout` for the server's answer.
#[track_caller]
fn shut_down_within(
    client: &Connection,
    server_thread: std::thread::JoinHandle<Result<(), String>>,
    timeout: Duration,
) {
    request_within(client, 1000, "shutdown", Value::Null, timeout);
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
#[track_caller]
fn open_document(client: &Connection, uri: &str, text: &str) {
    open_document_within(client, uri, text, TIMEOUT);
}

#[track_caller]
fn open_document_within(client: &Connection, uri: &str, text: &str, timeout: Duration) {
    notify(
        client,
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "newspeak", "version": 1, "text": text } }),
    );
    let _ = next_diagnostics_within(client, timeout);
}

#[track_caller]
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

#[track_caller]
fn breakable_lines(client: &Connection, id: i32, uri: &str) -> Value {
    request_within(
        client,
        id,
        "newspeak/breakableLines",
        json!({ "textDocument": { "uri": uri } }),
        STRESS_TIMEOUT,
    )
}

/// A 0-based `(line, character)` position.
type LineCharacter = (u32, u32);

/// A breakable line's entry: its 0-based line and its variants, each the
/// block literal's start and end and its first statement's position.
fn breakable_line(line: u32, variants: &[(LineCharacter, LineCharacter, LineCharacter)]) -> Value {
    let position =
        |(line, character): LineCharacter| json!({ "line": line, "character": character });
    json!({
        "line": line,
        "variants": variants
            .iter()
            .map(|&(start, end, statement)| json!({
                "range": { "start": position(start), "end": position(end) },
                "position": position(statement),
            }))
            .collect::<Vec<Value>>(),
    })
}

/// A statement of each kind: the superclass clause, a temporary's
/// initializer, a line holding a statement and an inlined block's first
/// one -- after non-ASCII text, so that a column counted in bytes rather
/// than UTF-16 units would be off -- and a closure whose literal begins a
/// line above its first statement; and two blocks beginning on one line,
/// its variants in order.
const BREAKABLE: &str = indoc! {"
    class Module = Object new () (
      public run = (
        | count ::= 0. |
        'äö' size > 0 ifTrue: [count:: 1].
        count > 0 ifTrue: [count:: 1] ifFalse: [count:: 2].
        {1. 2} do: [:each |
          count:: count + each].
        ^count
      )
    ) : ()
"};

/// `BREAKABLE`'s lines, every one moved down by `shift`.
fn breakable_lines_of_the_module(shift: u32) -> Value {
    json!([
        breakable_line(shift, &[]),
        breakable_line(2 + shift, &[]),
        breakable_line(
            3 + shift,
            &[((3 + shift, 26), (3 + shift, 37), (3 + shift, 27))]
        ),
        breakable_line(
            4 + shift,
            &[
                ((4 + shift, 22), (4 + shift, 33), (4 + shift, 23)),
                ((4 + shift, 43), (4 + shift, 54), (4 + shift, 44)),
            ]
        ),
        breakable_line(5 + shift, &[]),
        breakable_line(
            6 + shift,
            &[((5 + shift, 15), (6 + shift, 27), (6 + shift, 6))]
        ),
        breakable_line(7 + shift, &[]),
    ])
}

#[test]
fn breakable_lines_answer_each_statement_line_and_its_block_variants() {
    let (client, server_thread) = initialized_server_within(STRESS_TIMEOUT);
    open_document_within(&client, URI, BREAKABLE, STRESS_TIMEOUT);
    assert_eq!(
        breakable_lines(&client, 2, URI),
        json!({ "version": 1, "lines": breakable_lines_of_the_module(0) })
    );
    assert_eq!(
        breakable_lines(&client, 3, URI),
        json!({ "version": 1, "lines": breakable_lines_of_the_module(0) }),
        "asked again, the same"
    );
    shut_down_within(&client, server_thread, STRESS_TIMEOUT);
}

#[test]
fn a_document_that_does_not_parse_answers_null_lines() {
    let (client, server_thread) = initialized_server();
    let unparsed = "file:///C:/work/Unparsed.ns";
    open_document_within(
        &client,
        unparsed,
        indoc! {"
            class Module = () (
              public run = ( ^1
            ) : ()
        "},
        STRESS_TIMEOUT,
    );
    assert_eq!(
        breakable_lines(&client, 2, unparsed),
        json!({ "version": 1, "lines": null })
    );
    let unresolved = "file:///C:/work/Unresolved.ns";
    open_document_within(
        &client,
        unresolved,
        indoc! {"
            class Module = () (
              public run = ( ^outer Missing foo )
            ) : ()
        "},
        STRESS_TIMEOUT,
    );
    assert_eq!(
        breakable_lines(&client, 3, unresolved),
        json!({ "version": 1, "lines": null }),
        "nor one that does not resolve"
    );
    shut_down(&client, server_thread);
}

#[test]
fn breakable_lines_follow_the_document_s_changes() {
    let (client, server_thread) = initialized_server_within(STRESS_TIMEOUT);
    open_document_within(&client, URI, BREAKABLE, STRESS_TIMEOUT);
    assert_eq!(
        breakable_lines(&client, 2, URI)["lines"],
        breakable_lines_of_the_module(0)
    );
    notify(
        &client,
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": URI, "version": 7 },
            "contentChanges": [{ "text": format!("/// A line above.\n{BREAKABLE}") }],
        }),
    );
    let _ = next_diagnostics_within(&client, STRESS_TIMEOUT);
    assert_eq!(
        breakable_lines(&client, 3, URI),
        json!({ "version": 7, "lines": breakable_lines_of_the_module(1) })
    );
    shut_down_within(&client, server_thread, STRESS_TIMEOUT);
}

#[test]
fn breakable_lines_of_an_unknown_document_are_invalid_params() {
    let (client, server_thread) = initialized_server();
    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(2),
            "newspeak/breakableLines".to_string(),
            json!({ "textDocument": { "uri": "file:///never/opened.ns" } }),
        )))
        .expect("send");
    let Message::Response(response) = receive(&client) else {
        panic!("expected a response");
    };
    assert_eq!(
        response.response_result.err().map(|error| error.code),
        Some(lsp_server::ErrorCode::InvalidParams as i32)
    );
    shut_down(&client, server_thread);
}

/// A module whose one method answers a chain of `links` messages, which
/// nests as many sends.
fn a_chain_of(links: usize) -> String {
    indoc::formatdoc! {"
        class Module = Object new () (
          public chain = (
            ^1{links}
          )
        ) : ()
        ",
        links = " + 1".repeat(links),
    }
}

/// The compiler recurses along a chain of messages: a chain as long as
/// the cap is compiled and answered, in a debug build too, on the
/// worker's stack, and a longer one answers `null` rather than
/// overflowing a stack, which would abort the server.
#[test]
fn a_chain_past_the_cap_answers_null_lines_and_the_server_serves_on() {
    let (client, server_thread) = initialized_server_within(STRESS_TIMEOUT);
    let at_cap = "file:///C:/work/AtTheCap.ns";
    open_document_within(
        &client,
        at_cap,
        &a_chain_of(ns_analyzer::breakable::MAX_NESTED_SENDS),
        STRESS_TIMEOUT,
    );
    assert_eq!(
        breakable_lines(&client, 2, at_cap),
        json!({ "version": 1, "lines": [breakable_line(0, &[]), breakable_line(2, &[])] })
    );
    let past_cap = "file:///C:/work/PastTheCap.ns";
    open_document_within(
        &client,
        past_cap,
        &a_chain_of(ns_analyzer::breakable::MAX_NESTED_SENDS + 1),
        STRESS_TIMEOUT,
    );
    assert_eq!(
        breakable_lines(&client, 3, past_cap),
        json!({ "version": 1, "lines": null })
    );
    open_document_within(&client, URI, BREAKABLE, STRESS_TIMEOUT);
    assert_eq!(
        breakable_lines(&client, 4, URI),
        json!({ "version": 1, "lines": breakable_lines_of_the_module(0) }),
        "the server serves on"
    );
    shut_down_within(&client, server_thread, STRESS_TIMEOUT);
}

/// Each line of a breakable lines' answer and how many variants it has.
fn lines_and_variant_counts(lines: &Value) -> Vec<(u64, usize)> {
    lines
        .as_array()
        .expect("lines are answered")
        .iter()
        .map(|entry| {
            (
                entry["line"].as_u64().expect("a line"),
                entry["variants"].as_array().expect("variants").len(),
            )
        })
        .collect()
}

/// What a breakable lines' answer of `statements` in `text` says of each
/// line: the lines statements start on and the blocks whose first each
/// is.
fn expected_lines(statements: &[ns_runtime::BreakableStatement], text: &str) -> Vec<(u64, usize)> {
    let index = ns_tooling::lines::LineIndex::new(text);
    let mut lines: Vec<(u64, usize)> = Vec::new();
    for statement in statements {
        let line = u64::from(index.position(text, statement.statement.start).line);
        if lines.last().is_none_or(|(last, _)| *last != line) {
            lines.push((line, 0));
        }
        if let (Some(_), Some((_, variants))) = (statement.block, lines.last_mut()) {
            *variants += 1;
        }
    }
    lines
}

/// A document holding, byte for byte, the text of a shipped module, as
/// the JetBrains plugin opens the library `ns-next shipped-sources`
/// writes, is the shipped module: no diagnostic is published for it, and
/// its breakable lines are the statements the binary's own compile of the
/// module stops at (`Interpreter::shipped_breakable_statements`), which
/// `ns-cli`'s `debug_metadata` holds to where a breakpoint lands. The
/// shipped and the program-side answers are equal today, so this pins the
/// behaviour, not which path answers; `debug_metadata` and the `ModuleSource`
/// the shipped compile takes tell the paths apart.
#[test]
fn a_shipped_module_s_text_is_the_shipped_module() {
    let (client, server_thread) = initialized_server_within(STRESS_TIMEOUT);
    let mut interpreter = ns_runtime::Interpreter::bootstrap();
    let shipped: Vec<_> = ns_runtime::shipped_source_files()
        .into_iter()
        .filter(|file| ns_runtime::shipped_module_named_by_text(file.text).is_some())
        .collect();
    assert!(shipped.len() >= 7, "only {} shipped modules", shipped.len());
    for (id, file) in (2..).zip(shipped) {
        let uri = format!("file:///C:/library/{}", file.path);
        notify(
            &client,
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": "newspeak", "version": 1, "text": file.text } }),
        );
        let diagnostics = next_diagnostics_within(&client, STRESS_TIMEOUT);
        assert_eq!(diagnostics["diagnostics"], json!([]), "{}", file.name);
        let statements = interpreter
            .shipped_breakable_statements(file.name)
            .expect("a shipped module's statements");
        assert!(!statements.is_empty(), "{}", file.name);
        let answer = breakable_lines(&client, id, &uri);
        assert_eq!(
            lines_and_variant_counts(&answer["lines"]),
            expected_lines(&statements, file.text),
            "{}",
            file.name
        );
    }
    shut_down_within(&client, server_thread, STRESS_TIMEOUT);
}

/// A text one byte away from a shipped module's -- a line put above it
/// -- is an ordinary module, its breakable lines its own compile's.
#[test]
fn a_text_one_byte_from_a_shipped_module_s_is_an_ordinary_module() {
    let (client, server_thread) = initialized_server_within(STRESS_TIMEOUT);
    let kernel = ns_runtime::shipped_module_source("Kernel").expect("the kernel's text");
    let text = format!("\n{kernel}");
    let uri = "file:///C:/work/Kernel.ns";
    open_document_within(&client, uri, &text, STRESS_TIMEOUT);
    let module = ns_syntax::parse(&text).expect("the text parses");
    let resolved = ns_runtime::resolve(
        module,
        text.as_str().into(),
        ns_runtime::SourceOrigin::File("Kernel.ns".into()),
    )
    .expect("the text resolves as a program's module");
    let statements = ns_runtime::Interpreter::bootstrap().breakable_statements(&resolved);
    let answer = breakable_lines(&client, 2, uri);
    assert_eq!(
        lines_and_variant_counts(&answer["lines"]),
        expected_lines(&statements, &text)
    );
    shut_down_within(&client, server_thread, STRESS_TIMEOUT);
}

/// The diagnostics the server publishes for `text`, each its message, its
/// severity and its range.
fn published_diagnostics(text: &str) -> Vec<(String, u64, Value)> {
    let (client, server_thread) = initialized_server();
    notify(
        &client,
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": URI, "languageId": "newspeak", "version": 1, "text": text } }),
    );
    let published = next_diagnostics(&client);
    shut_down(&client, server_thread);
    published["diagnostics"]
        .as_array()
        .expect("an array")
        .iter()
        .map(|diagnostic| {
            (
                diagnostic["message"]
                    .as_str()
                    .expect("a message")
                    .to_string(),
                diagnostic["severity"].as_u64().expect("a severity"),
                diagnostic["range"].clone(),
            )
        })
        .collect()
}

/// The range of the first line's metadata comment, `(*` through `*)`.
fn first_line_comment_range(text: &str) -> Value {
    let line = text.lines().next().expect("a first line");
    let start = line.find("(*").expect("the comment") as u64;
    let end = line.find("*)").expect("its end") as u64 + 2;
    json!({ "start": { "line": 0, "character": start }, "end": { "line": 0, "character": end } })
}

/// `ns-next run` refuses a module whose `:module:` id ends in another name
/// than its class's (`docs/design/modules.md`, decision 2); the editor
/// shows the same message, as an error, on the metadata.
#[test]
fn a_module_id_not_ending_in_its_class_is_reported() {
    let text = indoc! {"
        (* :module: com.acme.json.Parser@2.1.0 *)
        class Json = Object new () ()
    "};
    assert_eq!(
        published_diagnostics(text),
        vec![(
            "`com.acme.json.Parser` does not end in the class's name `Json`".to_string(),
            1,
            first_line_comment_range(text)
        )]
    );
}

#[test]
fn a_malformed_version_is_reported() {
    let text = indoc! {"
        (* :module: com.acme.json.Json@2.1 *)
        class Json = Object new () ()
    "};
    let found = published_diagnostics(text);
    assert_eq!(found.len(), 1, "{found:?}");
    let (message, severity, range) = &found[0];
    assert!(message.starts_with("`2.1` is no version: "), "{message}");
    assert_eq!(*severity, 1, "an error");
    assert_eq!(*range, first_line_comment_range(text));
}

#[test]
fn a_reserved_id_is_reported() {
    let text = indoc! {"
        (* :module: newspeak.acme.Json *)
        class Json = Object new () ()
    "};
    assert_eq!(
        published_diagnostics(text),
        vec![(
            "`newspeak.acme.Json` lies in the namespace `newspeak.`, which belongs to the modules ns-next ships"
                .to_string(),
            1,
            first_line_comment_range(text)
        )]
    );
}

/// What a factory parameter expects is documentation (decision 7): checked
/// by nothing at run time, so a malformed one is a warning, and a library
/// may expect a shipped module.
#[test]
fn a_malformed_expectation_is_a_warning_and_a_shipped_one_is_none() {
    let text = indoc! {"
        class Reports usingPlatform: platform json: (* :module: com.acme.json.Json@2.x *) json = Object new () ()
    "};
    let found = published_diagnostics(text);
    assert_eq!(found.len(), 1, "{found:?}");
    let (message, severity, _) = &found[0];
    assert!(message.starts_with("`2.x` is no version: "), "{message}");
    assert_eq!(*severity, 2, "a warning");
    let malformed_id = text.replace("com.acme.json.Json@2.x", "com.acme..Json");
    let found = published_diagnostics(&malformed_id);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].0.starts_with("`com.acme..Json` is no module id"),
        "{}",
        found[0].0
    );
    let shipped = text.replace("com.acme.json.Json@2.x", "newspeak.core.CollectionsFactory");
    assert_eq!(published_diagnostics(&shipped), Vec::new());
}

/// The hover over the module's class names its id and version, and over a
/// factory parameter the module it expects.
#[test]
fn a_parameter_s_expectation_is_shown_on_hover() {
    let text = indoc! {"
        (* :module: com.acme.reports.Reports@1.0.0 *)
        class Reports usingPlatform: platform json: (* :module: com.acme.json.Json@2 *) json = Object new () ()
    "};
    let (client, server_thread) = initialized_server();
    open_document(&client, URI, text);
    let hover_at = |id: i32, name: &str| -> String {
        let line = text.lines().nth(1).expect("the class's line");
        let column = line.rfind(name).expect("the name") as u64 + 1;
        let hover = request(
            &client,
            id,
            "textDocument/hover",
            json!({ "textDocument": { "uri": URI }, "position": { "line": 1, "character": column } }),
        );
        hover["contents"]["value"]
            .as_str()
            .expect("a hover")
            .to_string()
    };
    assert_eq!(
        hover_at(2, " json = "),
        "```newspeak\njson\n```\n\n*Expects* `com.acme.json.Json@2`"
    );
    let class = hover_at(3, "Reports");
    assert!(
        class.ends_with("\n```\n\n*Module* `com.acme.reports.Reports@1.0.0`"),
        "{class}"
    );
    shut_down(&client, server_thread);
}

/// The server tells the binary's own source by the file its URI names: an
/// edited `Kernel.ns` at its place in a `core-library` directory declares
/// its reserved id by right, while a program's file elsewhere declaring
/// the same id is refused, as `ns-next run` refuses it.
#[test]
fn a_reserved_id_is_the_core_library_s_by_the_document_s_place() {
    let (client, server_thread) = initialized_server();
    let directory = scratch_directory("core-library-place");
    let kernel = ns_runtime::shipped_module_source("Kernel").expect("the kernel's text");
    let edited = format!("/// An edit.\n{kernel}");
    let reserved = |uri: &str| -> Vec<String> {
        notify(
            &client,
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": "newspeak", "version": 1, "text": edited } }),
        );
        next_diagnostics(&client)["diagnostics"]
            .as_array()
            .expect("an array")
            .iter()
            .filter_map(|diagnostic| diagnostic["message"].as_str())
            .filter(|message| message.contains("lies in the namespace"))
            .map(str::to_string)
            .collect()
    };
    let in_place = file_uri_of(&directory.join("core-library").join("Kernel.ns"));
    assert_eq!(reserved(&in_place), Vec::<String>::new());
    let elsewhere = file_uri_of(&directory.join("program").join("Kernel.ns"));
    assert_eq!(
        reserved(&elsewhere),
        vec![
            "`newspeak.core.Kernel` lies in the namespace `newspeak.`, which belongs to the modules ns-next ships"
                .to_string()
        ]
    );
    shut_down(&client, server_thread);
}
