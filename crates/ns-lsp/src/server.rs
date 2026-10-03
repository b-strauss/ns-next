//! The LSP message loop: one thread, synchronous. Lexing, parsing and
//! resolving one file takes milliseconds, so neither a debounce nor a worker
//! pool would buy anything, and one thread is what the thread-local symbol
//! interner needs anyway.

use std::collections::HashMap;
use std::error::Error;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, LogMessage,
    Notification as NotificationKind, PublishDiagnostics,
};
use lsp_types::request::{
    DocumentSymbolRequest, FoldingRangeRequest, Request as RequestKind, SemanticTokensFullRequest,
};
use lsp_types::{
    Diagnostic, DiagnosticSeverity, DocumentSymbolParams, DocumentSymbolResponse,
    FoldingRangeParams, FoldingRangeProviderCapability, LogMessageParams, MessageType, OneOf,
    PublishDiagnosticsParams, Range, SemanticTokens, SemanticTokensFullOptions,
    SemanticTokensOptions, SemanticTokensParams, SemanticTokensServerCapabilities,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};
use ns_syntax::Severity;

use crate::analysis::{analyze, Analysis};
use crate::document_symbols;
use crate::documents::LineIndex;
use crate::folding;
use crate::semantic_tokens;

pub type ServerResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// One open document and its analysis, computed once per version.
struct Document {
    text: String,
    line_index: LineIndex,
    analysis: Analysis,
}

impl Document {
    fn new(text: String) -> Document {
        let line_index = LineIndex::new(&text);
        let analysis = analyze(&text);
        Document {
            text,
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
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    return Ok(());
                }
                let response = answer(&documents, request);
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

fn answer(documents: &HashMap<String, Document>, request: Request) -> Response {
    match request.method.as_str() {
        SemanticTokensFullRequest::METHOD => semantic_tokens_of(documents, request),
        FoldingRangeRequest::METHOD => folding_ranges_of(documents, request),
        DocumentSymbolRequest::METHOD => document_symbols_of(documents, request),
        _ => Response::new_err(
            request.id,
            ErrorCode::MethodNotFound as i32,
            format!("ns-next-lsp does not implement {}", request.method),
        ),
    }
}

/// A document's folds; none for a document the client never opened or has
/// closed, as for its tokens.
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

/// A document's symbol tree; none for a document the client never opened or
/// has closed, as for its folds.
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
    let symbols = documents
        .get(params.text_document.uri.as_str())
        .map(|document| {
            document_symbols::encode(
                &document.analysis.symbols,
                &document.text,
                &document.line_index,
            )
        })
        .unwrap_or_default();
    Response::new_ok(request.id, DocumentSymbolResponse::Nested(symbols))
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
                Document::new(params.text_document.text),
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
            documents.insert(uri.as_str().to_string(), Document::new(change.text));
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

/// The diagnostics to publish for `uri`: its findings, or none once closed.
fn diagnostics_of(uri: &Uri, document: Option<&Document>) -> PublishDiagnosticsParams {
    let diagnostics = document.map_or_else(Vec::new, |document| {
        document
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
            .collect()
    });
    PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: None,
    }
}
