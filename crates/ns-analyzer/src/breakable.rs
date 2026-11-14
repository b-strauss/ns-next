//! The lines a line breakpoint can stand on, and the block bodies that
//! begin on each, for the custom request `newspeak/breakableLines`: the
//! JetBrains plugin decides `canPutAt` and a line breakpoint's variants
//! from them before any debug session runs, when no agent can be asked
//! (`docs/design/editor-support.md`, "Breakable lines";
//! `docs/design/debugger.md`, "A breakpoint names a source position").

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use lsp_types::{Range, Uri};
use ns_runtime::{BreakableStatement, Interpreter, SourceOrigin};
use serde_json::{json, Value};

use crate::analysis::panic_detail;
use crate::documents::LineIndex;
use crate::file_uri::file_path_of;

/// The method of the request.
pub const METHOD: &str = "newspeak/breakableLines";

/// The interpreter every document's breakable statements are compiled
/// in, bootstrapped by the first request that needs it rather than at the
/// server's start, so that an editor that never asks pays nothing, and
/// kept, since a bootstrap per request would cost more than the compile.
/// It instantiates the shipped core library and nothing else:
/// `Interpreter::breakable_statements` compiles the document's module
/// without loading or running it. The interpreter stays on the server's
/// thread, whose symbol interner it shares.
#[derive(Default)]
pub struct Breakable {
    interpreter: Option<Interpreter>,
}

impl Breakable {
    /// The `lines` of the request's answer for `text`: one entry per line
    /// a breakable statement starts on, in order, each with its variants
    /// -- the block literal's range and its first statement's position,
    /// for each statement starting on the line that is a block's first --
    /// or `null` for a text that does not parse or resolve, every line of
    /// which the plugin allows. A panic in the compile is an ns-next
    /// defect; it answers `null` too, and drops the interpreter, whose
    /// state it may have left half changed, for a new one at the next
    /// request.
    pub fn lines(&mut self, uri: &Uri, text: &str, line_index: &LineIndex) -> Value {
        let answered = catch_unwind(AssertUnwindSafe(|| {
            let module = ns_syntax::parse(text).ok()?;
            let resolved = ns_runtime::resolve(module, text.into(), origin_of(uri)).ok()?;
            let interpreter = self.interpreter.get_or_insert_with(Interpreter::bootstrap);
            Some(interpreter.breakable_statements(&resolved))
        }));
        match answered {
            Ok(Some(statements)) => encode(&statements, text, line_index),
            Ok(None) => Value::Null,
            Err(payload) => {
                self.interpreter = None;
                eprintln!(
                    "internal error in ns-next-analyzer answering {METHOD} for {}: {}; this is an ns-next defect, please report it",
                    uri.as_str(),
                    panic_detail(payload.as_ref())
                );
                Value::Null
            }
        }
    }
}

/// The origin a module compiled from `uri` names: the file, or the URI
/// itself for one that names no file. Nothing of the compile reads it
/// but a message, and nothing is loaded.
fn origin_of(uri: &Uri) -> SourceOrigin {
    SourceOrigin::File(file_path_of(uri).unwrap_or_else(|| PathBuf::from(uri.as_str())))
}

/// `statements`, sorted by start, grouped by the line each starts on.
fn encode(statements: &[BreakableStatement], text: &str, line_index: &LineIndex) -> Value {
    let mut lines: Vec<(u32, Vec<Value>)> = Vec::new();
    for statement in statements {
        let position = line_index.position(text, statement.statement.start);
        if lines.last().is_none_or(|(line, _)| *line != position.line) {
            lines.push((position.line, Vec::new()));
        }
        if let (Some(block), Some((_, variants))) = (statement.block, lines.last_mut()) {
            let range = Range::new(
                line_index.position(text, block.start),
                line_index.position(text, block.end),
            );
            variants.push(json!({ "range": range, "position": position }));
        }
    }
    Value::Array(
        lines
            .into_iter()
            .map(|(line, variants)| json!({ "line": line, "variants": variants }))
            .collect(),
    )
}
