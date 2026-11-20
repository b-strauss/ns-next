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

use crate::bytecode::{self, CompiledCode};
use crate::jit::CustomizationKey;

use super::names::NamedRoot;
use super::{CodeTier, TraceFilter};

/// What the sink's `listing` receives: the code of one body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    /// A body's bytecode, listed when its mixin was made.
    Bytecode(BytecodeListing),
    /// A body's machine code, listed when it was installed.
    Code(CodeListing),
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

/// A body's machine code as it was installed (decisions 4, 5 and 14): its
/// bytes as the assembler laid them out -- the data words zeroed, never
/// their values, which are addresses of the run -- with what a reader
/// needs to make sense of them, every offset counted from the body's
/// start. The host decodes the bytes; of two runs of one program, only
/// `base_address` differs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeListing {
    /// The body's name, as the trace names a body.
    pub body: String,
    /// Which compile of the body the code is.
    pub tier: CodeTier,
    /// An optimized copy's customization key: its receiver's class and,
    /// when the application it entered is another, that one's, as the
    /// trace's events name them; `None` for every other code.
    pub key: Option<(String, Option<String>)>,
    /// The architecture whose instructions the bytes are.
    pub architecture: Architecture,
    /// The address the body's first byte was installed at.
    pub base_address: u64,
    /// The body's bytes, its data words zeroed.
    pub bytes: Vec<u8>,
    /// The offset of the first data word, past the last instruction; the
    /// bytes' length for a body without data words.
    pub code_end: u32,
    /// Each data word's offset and what it holds, ascending by offset.
    pub words: Vec<(u32, DataWord)>,
    /// Where the code of each instruction starts, and where code out of
    /// the straight line does; ascending by offset.
    pub positions: Vec<(u32, Position)>,
    /// What a call or jump out of the body at each offset reaches;
    /// ascending by offset.
    pub annotations: Vec<(u32, Annotation)>,
    /// The scopes the positions name by number: a baseline body's one,
    /// `s0`, the body itself.
    pub scopes: Vec<ListedScope>,
    /// The offset a stopped frame's code stands at, in a frame's code the
    /// debugger answers; `None` in a listing printed at an install.
    pub mark: Option<u32>,
}

/// The architecture a listing's bytes are instructions of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Architecture {
    X64,
    Aarch64,
}

impl Architecture {
    /// The architecture's name, as a listing's head spells it.
    pub fn name(self) -> &'static str {
        match self {
            Architecture::X64 => "x64",
            Architecture::Aarch64 => "aarch64",
        }
    }
}

/// What the code from an offset on lowers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    /// The instruction at `program_counter` of the scope numbered `scope`.
    Instruction { scope: u32, program_counter: u32 },
    /// Code out of the straight line: the paths a check takes when it
    /// fails, the tails of backward jumps, a dispatching entry.
    OutOfLine,
}

/// A scope of a listing: the body whose instructions its positions name,
/// numbered as the positions name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedScope {
    /// The scope's number, 0 for the body's own.
    pub number: u32,
    /// The scope's body, as the trace names a body.
    pub body: String,
    /// The scope it is inlined into and the program counter of the send
    /// it is inlined at there; `None` for the body's own scope.
    pub parent: Option<(u32, u32)>,
    /// The body's instructions, one line per program counter, as
    /// `bytecode::instruction_lines` writes them.
    pub instructions: Vec<String>,
}

/// What a call or jump out of the body reaches, named so that a listing
/// holds no address of its run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Annotation {
    /// A call of the named leaf, a Rust function, through the leaf shim.
    Leaf(&'static str),
    /// A call or jump through the entry block's word of the named stub.
    Stub(&'static str),
    /// The exit of the named kind, which hands the instruction at
    /// `program_counter` to the interpreter.
    Exit {
        kind: &'static str,
        program_counter: u32,
    },
    /// A send's call through the cell of its site numbered `site`.
    Cell { site: u32 },
}

/// What a body's data word holds, named rather than shown: its value is
/// an address of the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataWord {
    /// The address of the optimization budget the code spends.
    Budget,
    /// The address of the body's dispatching rows.
    DispatchingRows,
    /// The address of the dispatching entry's shared tail.
    DispatchingTail,
    /// The twin's address of the `MakeClosure` at `program_counter`.
    TwinTransfer { program_counter: u32 },
    /// Any other: a closure's or an outer local's site.
    Other,
}

/// A listing as its install captured it, waiting for the flush that names
/// its body. Captured at the install rather than read at the flush, since
/// a twin compiled in a leaf is flushed only later, when its code may
/// have moved or been freed.
pub(crate) enum PendingListing {
    Code(PendingCode),
}

/// A `CodeListing`'s facts, its body and key not yet named.
pub(crate) struct PendingCode {
    pub(crate) tier: CodeTier,
    pub(crate) key: Option<CustomizationKey>,
    pub(crate) architecture: Architecture,
    pub(crate) base_address: u64,
    pub(crate) bytes: Vec<u8>,
    pub(crate) code_end: u32,
    pub(crate) words: Vec<(u32, DataWord)>,
    pub(crate) positions: Vec<(u32, Position)>,
    pub(crate) annotations: Vec<(u32, Annotation)>,
}

impl PendingListing {
    /// The customization key the listing names, if any.
    pub(crate) fn key(&self) -> Option<CustomizationKey> {
        match self {
            PendingListing::Code(code) => code.key,
        }
    }

    /// The listing of `code`, named `name`, its key named `key`.
    pub(crate) fn delivered(
        self,
        code: &CompiledCode,
        name: String,
        key: Option<(String, Option<String>)>,
    ) -> Listing {
        match self {
            PendingListing::Code(pending) => {
                let scope = ListedScope {
                    number: 0,
                    body: name.clone(),
                    parent: None,
                    instructions: bytecode::instruction_lines(code),
                };
                Listing::Code(CodeListing {
                    body: name,
                    tier: pending.tier,
                    key,
                    architecture: pending.architecture,
                    base_address: pending.base_address,
                    bytes: pending.bytes,
                    code_end: pending.code_end,
                    words: pending.words,
                    positions: pending.positions,
                    annotations: pending.annotations,
                    scopes: vec![scope],
                    mark: None,
                })
            }
        }
    }
}
