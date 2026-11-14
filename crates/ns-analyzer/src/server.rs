//! The LSP message loop: one thread, synchronous. Lexing, parsing and
//! resolving one file takes milliseconds, so neither a debounce nor a worker
//! pool would buy anything, and one thread is what the thread-local symbol
//! interner needs anyway.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::error::Error;
use std::io::Read;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, LogMessage,
    Notification as NotificationKind, PublishDiagnostics,
};
use lsp_types::request::{
    CodeActionRequest, DocumentSymbolRequest, FoldingRangeRequest, Formatting, HoverRequest,
    Request as RequestKind, SemanticTokensFullRequest,
};
use lsp_types::{
    CodeActionParams, CodeActionProviderCapability, Diagnostic, DiagnosticSeverity,
    DocumentFormattingParams, DocumentSymbolParams, DocumentSymbolResponse, FoldingRangeParams,
    FoldingRangeProviderCapability, HoverParams, HoverProviderCapability, LogMessageParams,
    MessageType, OneOf, PublishDiagnosticsParams, Range, SemanticTokens, SemanticTokensFullOptions,
    SemanticTokensOptions, SemanticTokensParams, SemanticTokensServerCapabilities,
    ServerCapabilities, TextDocumentIdentifier, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextEdit, Uri,
};
use ns_syntax::{Severity, Span};

use crate::analysis::{analyze, Analysis};
use crate::breakable::{self, Breakable};
use crate::code_actions;
use crate::document_symbols;
use crate::documents::LineIndex;
use crate::file_uri::file_path_of;
use crate::folding;
use crate::hover;
use crate::semantic_tokens;

pub type ServerResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// One open document and its analysis, computed once per version.
struct Document {
    text: String,
    /// The version the client last gave the text, which the breakable
    /// lines' answer names so that the plugin keeps it per version; 0 for
    /// a document read from disk, which has none. `breakable_lines_of`
    /// answers open documents only, never one `document_on_disk` read:
    /// routed through it, the answer would name the version 0.
    version: i32,
    line_index: LineIndex,
    analysis: Analysis,
    /// The `lines` of the breakable lines' answer, computed by the first
    /// request for them: each compiles the module.
    breakable_lines: OnceCell<serde_json::Value>,
    /// The edit applying every hint, computed by the first code-action
    /// request that needs it: each round of it parses the text afresh.
    whole_file_edit: OnceCell<Option<(Span, String)>>,
}

impl Document {
    fn new(text: String, version: i32) -> Document {
        let line_index = LineIndex::new(&text);
        let analysis = analyze(&text);
        Document {
            text,
            version,
            whole_file_edit: OnceCell::new(),
            breakable_lines: OnceCell::new(),
            line_index,
            analysis,
        }
    }
}

fn capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: semantic_tokens::legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: None,
                ..SemanticTokensOptions::default()
            },
        )),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        ..ServerCapabilities::default()
    }
}

/// Initializes over `connection`, serves until the client's `shutdown` and
/// `exit`, and returns.
pub fn run(connection: Connection) -> ServerResult<()> {
    connection.initialize(serde_json::to_value(capabilities())?)?;
    // Keyed by the URI's string form, not the `Uri` itself: `Uri` wraps
    // `fluent_uri::Uri<String>`, which caches parsed components behind a
    // `Cell`, so clippy's `mutable_key_type` refuses it as a hash key even
    // though its `Hash`/`Eq` only ever read `as_str()`.
    let mut documents: HashMap<String, Document> = HashMap::new();
    let mut breakable = Breakable::default();
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    return Ok(());
                }
                let response = answer(&documents, &mut breakable, request);
                connection.sender.send(Message::Response(response))?;
            }
            Message::Notification(notification) => match apply(&mut documents, notification) {
                Ok(Some(uri)) => {
                    let params = diagnostics_of(&uri, documents.get(uri.as_str()));
                    connection
                        .sender
                        .send(Message::Notification(Notification::new(
                            PublishDiagnostics::METHOD.to_string(),
                            params,
                        )))?;
                }
                Ok(None) => {}
                Err(message) => report_malformed(&connection, message)?,
            },
            Message::Response(_) => {}
        }
    }
    Ok(())
}

fn answer(
    documents: &HashMap<String, Document>,
    breakable: &mut Breakable,
    request: Request,
) -> Response {
    match request.method.as_str() {
        SemanticTokensFullRequest::METHOD => semantic_tokens_of(documents, request),
        FoldingRangeRequest::METHOD => folding_ranges_of(documents, request),
        DocumentSymbolRequest::METHOD => document_symbols_of(documents, request),
        Formatting::METHOD => formatting_of(documents, request),
        CodeActionRequest::METHOD => code_actions_of(documents, request),
        HoverRequest::METHOD => hover_of(documents, request),
        breakable::METHOD => breakable_lines_of(documents, breakable, request),
        _ => Response::new_err(
            request.id,
            ErrorCode::MethodNotFound as i32,
            format!("ns-next-analyzer does not implement {}", request.method),
        ),
    }
}

/// A document's folds; none for a document the client never opened or has
/// closed, as for its tokens. Unlike its symbols, neither is read from disk:
/// the platform asks for folds and tokens only of a file open in an editor,
/// which it has opened with the server first.
fn folding_ranges_of(documents: &HashMap<String, Document>, request: Request) -> Response {
    let params: FoldingRangeParams = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            return Response::new_err(
                request.id,
                ErrorCode::InvalidParams as i32,
                error.to_string(),
            )
        }
    };
    let ranges = documents
        .get(params.text_document.uri.as_str())
        .map(|document| {
            folding::encode(
                &document.analysis.folds,
                &document.text,
                &document.line_index,
            )
        })
        .unwrap_or_default();
    Response::new_ok(request.id, ranges)
}

/// The edit that formats an open document: one replacement of its whole text,
/// or none when it is formatted already or does not parse (a document with a
/// syntax error is left as the user wrote it). Like folds and tokens, only an
/// open editor buffer is formatted, never a file read from disk. The request's
/// options are ignored: the formatter has one style. A panic in the printer
/// answers no edit too (`formatted_guarded`).
fn formatting_of(documents: &HashMap<String, Document>, request: Request) -> Response {
    let params: DocumentFormattingParams = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            return Response::new_err(
                request.id,
                ErrorCode::InvalidParams as i32,
                error.to_string(),
            )
        }
    };
    let edits: Vec<TextEdit> = documents
        .get(params.text_document.uri.as_str())
        .and_then(|document| {
            let formatted = formatted_guarded(&document.text, ns_syntax::format)?;
            if formatted == document.text {
                return None;
            }
            let end = document
                .line_index
                .position(&document.text, u32::try_from(document.text.len()).ok()?);
            Some(TextEdit {
                range: Range::new(lsp_types::Position::new(0, 0), end),
                new_text: formatted,
            })
        })
        .into_iter()
        .collect();
    Response::new_ok(request.id, edits)
}

/// `text` as `formatter` writes it, or `None` when it does not parse or the
/// formatter panics. The printer is meant to be total over every module that
/// parses; a panic is therefore an ns-next defect, and like a panic in the
/// analysis (`analysis::guarded`) it must not take the server, and the
/// editor's support for every other file, down with it. The document is
/// then left as it is.
fn formatted_guarded<Failure>(
    text: &str,
    formatter: impl FnOnce(&str) -> Result<String, Failure>,
) -> Option<String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| formatter(text)))
        .ok()?
        .ok()
}

/// A document's symbol tree. An open document is answered from its cache: it
/// may hold text not yet saved. A document the client has not opened is read
/// from disk, when its URI names a file, and analysed afresh each time: the
/// platform sends `didOpen` only for a file open in an editor, but the
/// plugin's structure view asks for a file merely selected in the project
/// view. The server keeps nothing of it, since without `didOpen` it hears of
/// no change. A URI that names no file, or a file that cannot be read, has no
/// symbols.
fn document_symbols_of(documents: &HashMap<String, Document>, request: Request) -> Response {
    let params: DocumentSymbolParams = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            return Response::new_err(
                request.id,
                ErrorCode::InvalidParams as i32,
                error.to_string(),
            )
        }
    };
    let uri = &params.text_document.uri;
    let encode = |document: &Document| {
        document_symbols::encode(
            &document.analysis.symbols,
            &document.text,
            &document.line_index,
        )
    };
    let symbols = match documents.get(uri.as_str()) {
        Some(document) => encode(document),
        None => document_on_disk(uri)
            .map(|document| encode(&document))
            .unwrap_or_default(),
    };
    Response::new_ok(request.id, DocumentSymbolResponse::Nested(symbols))
}

/// The file `uri` names, read and analysed as `didOpen` would have it, or
/// `None` when it names no file or the file cannot be read.
fn document_on_disk(uri: &Uri) -> Option<Document> {
    let path = file_path_of(uri)?;
    // Asked of the path before it is opened: opening a FIFO blocks until a
    // writer comes, and reading a device such as `/dev/zero` never ends,
    // either of which would stall the one thread every document is served
    // on. A file longer than a source may be is refused before it is read.
    let metadata = std::fs::metadata(&path).ok()?;
    if !readable_as_source(metadata.is_file(), metadata.len()) {
        return None;
    }
    // Bounded again while reading, for a file that grew since it was asked.
    let limit = u64::from(u32::MAX) + 1;
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .ok()?
        .take(limit)
        .read_to_end(&mut bytes)
        .ok()?;
    if !readable_as_source(true, bytes.len() as u64) {
        return None;
    }
    // UTF-8, as every Newspeak source of the project is; lossy rather than
    // refused, so a stray invalid byte costs one character of the tree, not
    // all of it, as the editor still shows the file. Only invalid text is
    // copied.
    let mut text = String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
    // The editor's document has no byte-order mark; with one, every position
    // on the first line would be one unit off from the editor's.
    if text.starts_with('\u{feff}') {
        text.drain(..'\u{feff}'.len_utf8());
    }
    Some(Document::new(text, 0))
}

/// Whether a file of `length` bytes, a regular file or not, may be read as a
/// source: a regular file no longer than `ns_syntax::source_too_long`
/// accepts, so that nothing is read that the parse would refuse anyway.
fn readable_as_source(is_file: bool, length: u64) -> bool {
    is_file
        && usize::try_from(length).is_ok_and(|length| ns_syntax::source_too_long(length).is_none())
}

fn semantic_tokens_of(documents: &HashMap<String, Document>, request: Request) -> Response {
    let params: SemanticTokensParams = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            return Response::new_err(
                request.id,
                ErrorCode::InvalidParams as i32,
                error.to_string(),
            )
        }
    };
    // A document the client never opened -- or already closed -- has no
    // tokens; answering an empty set keeps the client's colours as they are.
    let data = documents
        .get(params.text_document.uri.as_str())
        .map(|document| {
            semantic_tokens::encode(
                &document.analysis.classified,
                &document.text,
                &document.line_index,
            )
        })
        .unwrap_or_default();
    Response::new_ok(
        request.id,
        SemanticTokens {
            result_id: None,
            data,
        },
    )
}

/// The hover over the name at a position of an open document: the
/// declaration it names in that document, if the classifier placed it at
/// one (`hover.rs`). None for a document never opened, as for its folds,
/// and for a position outside the document.
fn hover_of(documents: &HashMap<String, Document>, request: Request) -> Response {
    let params: HoverParams = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            return Response::new_err(
                request.id,
                ErrorCode::InvalidParams as i32,
                error.to_string(),
            )
        }
    };
    let position = params.text_document_position_params;
    let hover = documents
        .get(position.text_document.uri.as_str())
        .and_then(|document| {
            let offset = document
                .line_index
                .offset(&document.text, position.position)?;
            hover::hover(
                &document.analysis.references,
                &document.analysis.declarations,
                offset,
                &document.text,
                &document.line_index,
            )
        });
    Response::new_ok(request.id, hover)
}

/// The lines of an open document a line breakpoint can stand on, with
/// the version they were computed for (`breakable.rs`). Unlike its folds,
/// a document never opened is an error rather than an empty answer: an
/// empty set of lines would forbid every breakpoint in the file, where
/// the plugin, told of the error, allows every line.
fn breakable_lines_of(
    documents: &HashMap<String, Document>,
    breakable: &mut Breakable,
    request: Request,
) -> Response {
    let identifier: Result<TextDocumentIdentifier, _> = serde_json::from_value(
        request
            .params
            .get("textDocument")
            .cloned()
            .unwrap_or_default(),
    );
    let identifier = match identifier {
        Ok(identifier) => identifier,
        Err(error) => {
            return Response::new_err(
                request.id,
                ErrorCode::InvalidParams as i32,
                format!("malformed {} request: {error}", breakable::METHOD),
            )
        }
    };
    let Some(document) = documents.get(identifier.uri.as_str()) else {
        return Response::new_err(
            request.id,
            ErrorCode::InvalidParams as i32,
            format!(
                "{} asks of a document never opened: {}",
                breakable::METHOD,
                identifier.uri.as_str()
            ),
        );
    };
    let lines = document
        .breakable_lines
        .get_or_init(|| breakable.lines(&identifier.uri, &document.text, &document.line_index));
    Response::new_ok(
        request.id,
        serde_json::json!({ "version": document.version, "lines": lines }),
    )
}

/// Applies a document notification; answers the URI whose diagnostics must be
/// published, or `None` for a notification that changes nothing. A
/// notification whose params do not match the schema is answered as `Err`
/// with what was wrong, for `report_malformed`, and changes nothing.
fn apply(
    documents: &mut HashMap<String, Document>,
    notification: Notification,
) -> Result<Option<Uri>, String> {
    match notification.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params = params_of::<DidOpenTextDocument>(notification)?;
            let uri = params.text_document.uri;
            documents.insert(
                uri.as_str().to_string(),
                Document::new(params.text_document.text, params.text_document.version),
            );
            Ok(Some(uri))
        }
        DidChangeTextDocument::METHOD => {
            let params = params_of::<DidChangeTextDocument>(notification)?;
            let uri = params.text_document.uri;
            // Full synchronisation: the last change carries the whole text.
            let Some(change) = params.content_changes.into_iter().last() else {
                return Ok(None);
            };
            documents.insert(
                uri.as_str().to_string(),
                Document::new(change.text, params.text_document.version),
            );
            Ok(Some(uri))
        }
        DidCloseTextDocument::METHOD => {
            let params = params_of::<DidCloseTextDocument>(notification)?;
            let uri = params.text_document.uri;
            documents.remove(uri.as_str());
            Ok(Some(uri))
        }
        _ => Ok(None),
    }
}

/// A notification's params as the schema of `Kind` has them, or what was
/// wrong with them.
fn params_of<Kind: NotificationKind>(notification: Notification) -> Result<Kind::Params, String> {
    serde_json::from_value(notification.params)
        .map_err(|error| format!("malformed {} notification: {error}", Kind::METHOD))
}

/// Reports a malformed notification to stderr and to the client. A
/// notification has no id and so no response an error could travel in
/// (LSP's base protocol), so the client hears of it through
/// `window/logMessage` at type `Error`: it lands in the client's log for the
/// server, beside the stderr line, without interrupting the user the way
/// `window/showMessage` would for what is a defect of the client, not of the
/// user's text. The server passes over the message rather than ending -- the
/// one message lost is better than every document already open going dark.
fn report_malformed(connection: &Connection, message: String) -> ServerResult<()> {
    eprintln!("{message}");
    connection
        .sender
        .send(Message::Notification(Notification::new(
            LogMessage::METHOD.to_string(),
            LogMessageParams {
                typ: MessageType::ERROR,
                message,
            },
        )))?;
    Ok(())
}

/// The diagnostics to publish for `uri`: its findings and its hints, or
/// none once closed.
fn diagnostics_of(uri: &Uri, document: Option<&Document>) -> PublishDiagnosticsParams {
    let diagnostics =
        document.map_or_else(Vec::new, |document| {
            let mut diagnostics: Vec<Diagnostic> = document
                .analysis
                .findings
                .iter()
                .map(|finding| Diagnostic {
                    range: Range::new(
                        document
                            .line_index
                            .position(&document.text, finding.span.start),
                        document
                            .line_index
                            .position(&document.text, finding.span.end),
                    ),
                    severity: Some(match finding.severity {
                        Severity::Error => DiagnosticSeverity::ERROR,
                        Severity::Warning => DiagnosticSeverity::WARNING,
                    }),
                    source: Some("ns-next".to_string()),
                    message: finding.message.clone(),
                    ..Diagnostic::default()
                })
                .collect();
            diagnostics.extend(
                document.analysis.hints.iter().map(|hint| {
                    code_actions::diagnostic(hint, &document.text, &document.line_index)
                }),
            );
            diagnostics
        });
    PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: None,
    }
}

/// The code actions at a range of an open document: its hints' quick fixes
/// and the fix for the whole file. None for a document never opened, as
/// for its folds.
fn code_actions_of(documents: &HashMap<String, Document>, request: Request) -> Response {
    let params: CodeActionParams = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            return Response::new_err(
                request.id,
                ErrorCode::InvalidParams as i32,
                error.to_string(),
            )
        }
    };
    let uri = &params.text_document.uri;
    let actions = documents
        .get(uri.as_str())
        .map(|document| {
            code_actions::actions(
                uri,
                &document.analysis.hints,
                &document.text,
                &document.line_index,
                params.range,
                || {
                    document
                        .whole_file_edit
                        .get_or_init(|| code_actions::whole_file_edit(&document.text))
                        .clone()
                },
            )
        })
        .unwrap_or_default();
    Response::new_ok(request.id, actions)
}

#[cfg(test)]
mod tests {
    use super::{formatted_guarded, readable_as_source};

    #[test]
    fn a_panic_in_the_printer_answers_no_formatted_text() {
        let formatted = formatted_guarded("anything", |_| -> Result<String, ()> {
            panic!("the printer broke")
        });
        assert_eq!(formatted, None);
        assert_eq!(
            formatted_guarded("text", |text| Ok::<_, ()>(text.to_uppercase())),
            Some("TEXT".to_string())
        );
        assert_eq!(formatted_guarded("text", |_| Err::<String, _>(())), None);
    }

    #[test]
    fn only_a_regular_file_a_source_may_hold_is_read() {
        assert!(readable_as_source(true, 0));
        assert!(readable_as_source(true, u64::from(u32::MAX)));
        // A four-gigabyte file is not written to test this; the bound is
        // checked on the length the file's metadata answers.
        assert!(!readable_as_source(true, u64::from(u32::MAX) + 1));
        assert!(
            !readable_as_source(false, 10),
            "a directory, FIFO or device"
        );
    }
}
