//! The code a run makes, as listings the trace's sink receives
//! (`docs/design/developer-tool.md`, decisions 10 to 14, "Stage 2:
//! printing code"). The runtime formats a body's bytecode, which it alone
//! reads; the host formats everything else.
//!
//! A body's bytecode is listed when its mixin is made, at the first flush
//! after the mixin joined the heap's tables: the bodies the trace's names
//! index for the first time (`BodyNames::index_new_sides`) are the bodies
//! made since the last flush, so that the boot's and the program's load's
//! are listed when the host installs its sink, a shipped module built on
//! first demand and an installed edit's new versions at the next flush.
//! Nothing is tested at a push: the listing costs nothing where nothing
//! asked for it.

use crate::bytecode;

use super::TraceFilter;
use super::names::NamedRoot;

/// What the sink's `listing` receives: the code of one body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    /// A body's bytecode, listed when its mixin was made.
    Bytecode(BytecodeListing),
}

/// A body's bytecode: its name, as the trace names a body
/// (`Demo/Account>>deposit:`), and `bytecode::disassemble`'s text of it,
/// the blocks it makes nested under their `MakeClosure`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BytecodeListing {
    pub body: String,
    pub text: String,
}

/// The bytecode listing of each of `roots` that `filter` selects, in
/// order. A block is listed with the root body that makes it, never alone.
pub(crate) fn bytecode_listings(roots: Vec<NamedRoot>, filter: &TraceFilter) -> Vec<Listing> {
    roots
        .into_iter()
        .filter(|root| filter.matches(&root.name))
        .map(|NamedRoot { code, name }| {
            let text = bytecode::disassemble(&code, &name);
            Listing::Bytecode(BytecodeListing { body: name, text })
        })
        .collect()
}
