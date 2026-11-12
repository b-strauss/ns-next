//! A body's debugger metadata: its statement starts, its sends and its
//! named variables (`docs/design/debugger.md`, decisions 4, 10 and 13).
//! Never stored with the body: the debugger compiles the body's source
//! again with a `MetadataCollector`, and the lowerer records each body's
//! tables on the side as it finishes it (decision 5).

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;

use ns_syntax::Span;

use crate::bytecode::{CallSite, CompiledCode};
use crate::Symbol;

pub(crate) mod again;
mod cache;

pub(crate) use cache::Cache;

/// The tables of one compiled body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyMetadata {
    /// Every statement start in emission order: the body's own statements,
    /// its local, slot and simultaneous initializers and prologue entries
    /// (wherever `statement_program_counter` gets a range), and every
    /// statement of an inlined block.
    pub(crate) statements: Box<[StatementEntry]>,
    /// Every send, `SendMirror` included, by ascending index.
    pub(crate) sends: Box<[SendEntry]>,
    /// The body's own variables first (parameters, then temporaries, in
    /// resolver order), then each enclosing scope's, outward.
    pub(crate) variables: Box<[VariableEntry]>,
    /// Where a factory's frame stands: its class header's factory message
    /// pattern, or the class's name for a header that writes none, in the
    /// module's current text (decision 12). A factory's body has no source
    /// and no statement start, so this is its one position. `None` for
    /// every body the lowerers compile.
    pub(crate) factory_pattern: Option<Span>,
}

/// A place a line breakpoint or a step stops at: the instructions one
/// statement compiled to, and the statement's source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatementEntry {
    pub(crate) instructions: Range<u32>,
    pub(crate) span: Span,
}

/// One send instruction and the source it was lowered from: what smart
/// step into offers, and what a frame standing at the send highlights.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SendEntry {
    /// The send instruction's index in its body.
    pub(crate) index: u32,
    /// `None` for a `SendMirror`, which has no call site.
    pub(crate) call_site: Option<CallSite>,
    /// `None` where nothing in the source spells the receiver: an
    /// implicit-receiver send and a setter assignment.
    pub(crate) receiver: Option<Span>,
    /// The selector's tokens (`ast::MessageSend::selector_spans`); the
    /// identifier's span for a unary send kept as an identifier and for a
    /// setter assignment.
    pub(crate) selector: Box<[Span]>,
}

/// A named variable in scope over the whole body: an inlined block
/// declares none, so every variable a body names is in scope from its
/// start, `nil` until written (decision 13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VariableEntry {
    pub(crate) name: Symbol,
    /// The lexical distance in frame scopes from the body to the scope
    /// that declares the variable: 0 for the body's own, inlined scopes
    /// not counted.
    pub(crate) depth: u16,
    pub(crate) place: VariablePlace,
}

/// Where a frame of the body reads a variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VariablePlace {
    /// In this frame: a parameter, a temporary or a value it was handed.
    Register(u16),
    /// In the frame `hops` closures' captured frames out.
    Outer { hops: u16, register: u16 },
    /// In a slot of an indirection vector.
    Vector { vector: VectorHolder, slot: u16 },
}

/// The register an indirection vector is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VectorHolder {
    Register(u16),
    Outer { hops: u16, register: u16 },
}

/// What one compilation collected, each body's tables keyed by the body
/// (`EncodedBody::identity`), so that a family's blocks, which the
/// compilation hands back only inside their creators' `MakeClosure`s, are
/// found by the code itself.
#[derive(Default)]
pub(crate) struct MetadataCollector {
    bodies: RefCell<HashMap<usize, BodyMetadata>>,
}

impl MetadataCollector {
    pub(crate) fn new() -> MetadataCollector {
        MetadataCollector::default()
    }

    /// Records `metadata` as the tables of `code`, which was just
    /// compiled.
    pub(crate) fn record(&self, code: &CompiledCode, metadata: BodyMetadata) {
        self.bodies
            .borrow_mut()
            .insert(code.body().identity(), metadata);
    }

    /// Moves the entry of `from` to `onto`: a body cloned after it was
    /// compiled (the kernel's unwind marker).
    pub(crate) fn rekey(&self, from: &CompiledCode, onto: &CompiledCode) {
        let mut bodies = self.bodies.borrow_mut();
        if let Some(metadata) = bodies.remove(&from.body().identity()) {
            bodies.insert(onto.body().identity(), metadata);
        }
    }

    /// The tables of `code`, taken out of the collector; `None` for a body
    /// this collection did not compile.
    pub(crate) fn take(&self, code: &CompiledCode) -> Option<BodyMetadata> {
        self.bodies.borrow_mut().remove(&code.body().identity())
    }
}

#[cfg(test)]
mod tests;
