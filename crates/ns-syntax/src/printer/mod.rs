//! The pretty printer: a module lowered to a [`Document`], then laid out.

mod declarations;
#[expect(
    dead_code,
    reason = "the parts for comments and for breaking expressions wait for those to be printed"
)]
mod document;
mod expressions;

use crate::ast::Module;
use crate::diagnostic::Diagnostic;
use document::layout;

/// The width every printed line is held to, in `char`s.
const WIDTH: u32 = 100;

/// How the printer ends its lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::min_ident_chars,
    reason = "`Lf` and `Cr` are the names line endings go by"
)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    /// The first line ending of `source`, `Lf` when it has none.
    pub fn of(source: &str) -> LineEnding {
        let bytes = source.as_bytes();
        match bytes.iter().position(|byte| matches!(byte, b'\n' | b'\r')) {
            Some(index) if bytes[index] == b'\n' => LineEnding::Lf,
            Some(index) if bytes.get(index + 1) == Some(&b'\n') => LineEnding::CrLf,
            Some(_) => LineEnding::Cr,
            None => LineEnding::Lf,
        }
    }

    /// The characters that end a line.
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }
}

/// `module` printed in the one layout this printer has
/// (`docs/design/formatting.md`), ending with one line ending.
pub fn print_module(module: &Module, line_ending: LineEnding) -> String {
    let document = declarations::module(module);
    let mut printed = layout(&document, WIDTH, line_ending.as_str());
    // The document ends at the root class's last token, never at a line
    // break, so the one final line ending is added here.
    printed.push_str(line_ending.as_str());
    printed
}

/// `source` reprinted, or the diagnostics refusing it: a source with any
/// error is never printed.
pub fn format(source: &str) -> Result<String, Vec<Diagnostic>> {
    let module = crate::parse(source)?;
    Ok(print_module(&module, LineEnding::of(source)))
}
