//! A body's debugger metadata: its statement starts, its sends and its
//! named variables (`docs/design/debugger.md`, decisions 4, 10 and 13).
//! Never stored with the body: the debugger compiles the body's source
//! again with a `MetadataCollector`, and the lowerer records each body's
//! tables on the side as it finishes it (decision 5).

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use ns_syntax::Span;

use crate::bytecode::{CallSite, CompiledCode};
use crate::Symbol;

pub(crate) mod again;
pub(crate) mod breakable;
mod cache;
mod check;

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
    /// Every send that takes a block literal as its receiver or an
    /// argument, inlined or not, in emission order: what a step out of
    /// such a block leaves (`docs/design/debugger.md`, decision 17).
    pub(crate) block_sends: Box<[BlockSendEntry]>,
    /// The body's own variables first (parameters, then temporaries, in
    /// resolver order), then each enclosing scope's, outward.
    pub(crate) variables: Box<[VariableEntry]>,
    /// Where a factory's frame stands: its class header's factory message
    /// pattern, or the class's name for a header that writes none, in the
    /// module's current text (decision 12). A factory's body has no source
    /// and no statement start, so this is its one position. `None` for
    /// every body the lowerers compile.
    pub(crate) factory_pattern: Option<Span>,
    /// Where an initializer's frame stands at its header stop, its entry
    /// before its first statement that a step into a superclass clause or
    /// a `<:` clause's message makes: its declaration's header, as the
    /// factory's `factory_pattern` gives it (decision 12). Set when the
    /// level is compiled again, for an initializer body itself, not its
    /// blocks; `None` for every other body.
    pub(crate) header_pattern: Option<Span>,
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
    /// The call, from its receiver -- or its selector, where the receiver
    /// is implicit or a cascade's later message's, which the source spells
    /// apart -- to its last argument; a setter assignment's whole
    /// assignment: what names the value it answered
    /// (`docs/design/debugger.md`, "The return value of the last call").
    pub(crate) call: Span,
    /// Whether a receiver spelled apart from the call -- a cascade's --
    /// is named in parentheses before it: a binary or keyword send
    /// written without them.
    pub(crate) parenthesize_receiver: bool,
    /// Whether the send is a setter assignment's, whose answer -- the
    /// receiver -- is no value a step reports: the assigned value's call
    /// stays the last.
    pub(crate) setter: bool,
}

/// A send that takes block literals, the compiler's inlined forms among
/// them -- `ifTrue:`, `and:`, `whileTrue:`, a mirror send's failure
/// block -- and where a frame of the body stands while each of its
/// blocks runs: what a step out taken in one of them leaves (decision
/// 17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockSendEntry {
    pub(crate) blocks: Box<[TakenBlock]>,
    /// Whether the send is one of the forms the optimized lowering
    /// inlines -- the reference lowering's `SendInlinable` and its loop
    /// among them -- which a step out leaves for the next statement start
    /// after it, as a step over of the whole form would; a step out of a
    /// real send's block stops at `after`, the send's return point.
    pub(crate) inlined: bool,
    /// The index after the send's whole lowering: after an inlined form's
    /// last instruction, its join or its loop's exit, and after a real
    /// send, the instruction after it.
    pub(crate) after: u32,
}

/// One block literal a send takes, by its source -- the `source` of the
/// closure a `MakeClosure` makes of it -- and where it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TakenBlock {
    pub(crate) span: Span,
    pub(crate) place: BlockPlace,
}

/// Where a frame of the body stands while a block it took runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlockPlace {
    /// Lowered inline: the block's statements are these instructions of
    /// the body itself.
    Inline(Range<u32>),
    /// Run as a closure by the send at `send` -- a real send, an inlined
    /// form's fallback send, a reference-lowered loop's `value` -- which
    /// the frame stands at while the block's frame runs above, the closure
    /// made by the `MakeClosure` at `closure`: an index, which names the
    /// running body's block as well as the body compiled again, whose
    /// spans an install may have moved.
    Called { send: u32, closure: u32 },
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

    /// Drops the tables of `block` and of every block it creates,
    /// transitively: the blocks of a body past a compiler limit, which
    /// were recorded as they were finished and are dropped with their
    /// creator. An entry left under a dropped body's identity would be
    /// answered for the next body allocated there that records none -- a
    /// body past a limit -- so the collector holds the tables of no body
    /// but those the compilation still holds. A worklist, not a recursion,
    /// as every walk of a family is.
    pub(crate) fn forget_family(&self, block: &Rc<CompiledCode>) {
        let mut bodies = self.bodies.borrow_mut();
        let mut pending = vec![Rc::clone(block)];
        while let Some(body) = pending.pop() {
            bodies.remove(&body.body().identity());
            pending.extend(
                body.body()
                    .created_blocks()
                    .map(|created| Rc::clone(created.code)),
            );
        }
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

    /// Whether the collector holds no body's tables.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.bodies.borrow().is_empty()
    }
}

/// A body's address, which names it while it lives: what the pairing,
/// the cache and the check key a running body by.
fn address(code: &Rc<CompiledCode>) -> usize {
    Rc::as_ptr(code).addr()
}

#[cfg(test)]
mod tests;
