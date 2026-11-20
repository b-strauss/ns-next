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

use std::rc::Rc;

use crate::bytecode::{self, CompiledCode};
use crate::interpreter::Mutator;
use crate::jit::CustomizationKey;
use crate::mixin::DispatchId;

use super::names::{BodyNames, NamedRoot};
use super::{CodeTier, CompileRefusal, InlineDecline, TraceFilter, class_name_of_id};

/// What the sink's `listing` receives: the code of one body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    /// A body's bytecode, listed when its mixin was made.
    Bytecode(BytecodeListing),
    /// A body's machine code, listed when it was installed.
    Code(CodeListing),
    /// The inlining plan of an optimizing compile, listed once its outcome
    /// is known.
    Inlining(InliningListing),
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
    /// The offset past the last instruction: the bytes from there to the
    /// first data word align the words and are no instruction; the bytes'
    /// length for a body without data words.
    pub code_end: u32,
    /// Each data word's offset and what it holds, ascending by offset.
    pub words: Vec<(u32, DataWord)>,
    /// Where the code of each instruction starts, and where code out of
    /// the straight line does; ascending by offset.
    pub positions: Vec<(u32, Position)>,
    /// What a call or jump out of the body at each offset reaches;
    /// ascending by offset.
    pub annotations: Vec<(u32, Annotation)>,
    /// The scopes the positions name by number, `s0` the body itself: a
    /// baseline body's only one, and for an optimized copy each scope its
    /// code lays out, numbered from 0 without a gap, each after the scope
    /// it is inlined into -- the tree `--print-inlining` prints.
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

/// The inlining plan of one optimizing compile of a body that made one,
/// installed or refused (decision 13): the tree of the scopes its code
/// lays out, numbered as a code listing's positions name them, the arms of
/// its type-cases, the callees it sent rather than inline, and the
/// compile's outcome, so that a body that stayed unoptimized shows the
/// plan that failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InliningListing {
    /// The compiled body's name, as the trace names a body.
    pub body: String,
    /// The copy's customization key, named as a code listing's is; `None`
    /// for a body's one uncustomized copy.
    pub key: Option<(String, Option<String>)>,
    /// The scopes the code lays out, `s0` the body's own, each after the
    /// scope it is inlined into; their `instructions` empty, since the
    /// plan is printed without its code.
    pub scopes: Vec<ListedScope>,
    /// Each arm of a type-case in a scope of `scopes`, in the order of its
    /// scope and program counter, then of the cache.
    pub arms: Vec<ListedArm>,
    /// Each callee a send of a scope of `scopes` could have inlined and
    /// the plan sent instead, in the order the plan decided them.
    pub declined: Vec<ListedDecline>,
    /// Whether the copy was installed.
    pub outcome: InliningOutcome,
}

/// An arm of a type-case: at the send at `program_counter` of the scope
/// numbered `scope`, the receiver whose class `receiver` names -- as the
/// trace names a key's, `#<id>` for an id no application holds any longer
/// -- runs `inlined`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedArm {
    pub scope: u32,
    pub program_counter: u32,
    pub receiver: String,
    pub inlined: ArmKind,
}

/// What an arm runs for its receiver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArmKind {
    /// The callee's body, inlined as the scope of this number.
    Scope(u32),
    /// What the callee does, expressed inline whatever its weight, named
    /// in kebab case: `getter`, `setter`, `lazy-getter`, `quick` (a
    /// constant, `self` or an argument answered), `builtin-class`,
    /// `value-of-parameter`, `cull-parameter`, and for a leaf `integer`,
    /// `identity`, `class`, `indexed`, `float` or `primitive`.
    Always(&'static str),
}

/// A callee the plan sent rather than inline: at the send at
/// `program_counter` of the scope numbered `scope`, the body `callee` --
/// as the trace names a body, `None` for a callee that is no body --
/// declined for `reason`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedDecline {
    pub scope: u32,
    pub program_counter: u32,
    pub callee: Option<String>,
    pub reason: InlineDecline,
}

/// What became of an optimizing compile's copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InliningOutcome {
    /// Installed, taking `bytes` of the code zone.
    Installed { bytes: usize },
    /// Refused, for the reason the trace's `optimize` category gives.
    Refused(CompileRefusal),
}

/// What a call or jump out of the body reaches, named so that a listing
/// holds no address of its run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Annotation {
    /// A call of the named leaf, a Rust function, through the leaf shim.
    Leaf(&'static str),
    /// A call or jump through the entry block's word of the named stub,
    /// or the instruction that selects that stub's address for a call
    /// shared with another path -- a send's load of the polymorphic stub,
    /// which the call its cell's hit makes too.
    Stub(&'static str),
    /// The exit of the named kind, which hands the instruction at
    /// `program_counter` to the interpreter.
    Exit {
        kind: &'static str,
        program_counter: u32,
    },
    /// A send's call through the cell of its site numbered `site`.
    Cell { site: u32 },
    /// An indirect call or jump to a target that is no stub, leaf or
    /// cell: the callee a leaf decided (`initializer-callee`,
    /// `forcing-callee`), or the body's twin a transfer goes on in
    /// (`twin`).
    Target(&'static str),
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
    Inlining(PendingInlining),
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
    /// The scopes an optimized copy inlined, `s1` on; empty for every
    /// other code, whose one scope is its body's.
    pub(crate) scopes: Vec<PendingScope>,
}

/// An inlined scope of a listing, its body not yet named: held as the
/// body itself, never by its code id, which a full collection before the
/// flush may give another body.
pub(crate) struct PendingScope {
    pub(crate) number: u32,
    pub(crate) code: Rc<CompiledCode>,
    pub(crate) parent: Option<(u32, u32)>,
}

/// An `InliningListing`'s facts, its bodies and receivers not yet named.
pub(crate) struct PendingInlining {
    pub(crate) key: Option<CustomizationKey>,
    /// The scopes the plan inlined, `s1` on.
    pub(crate) scopes: Vec<PendingScope>,
    pub(crate) arms: Vec<PendingArm>,
    pub(crate) declined: Vec<PendingDecline>,
    pub(crate) outcome: InliningOutcome,
}

/// A `ListedArm`, its receiver held by its dispatch id until the flush
/// names it, or named before when a full collection reclaims the id
/// (`trace_events::name_freed_keys`): named at the flush, the id would be
/// no application's, or another's minted it since.
pub(crate) struct PendingArm {
    pub(crate) scope: u32,
    pub(crate) program_counter: u32,
    pub(crate) receiver: DispatchId,
    pub(crate) named: Option<String>,
    pub(crate) inlined: ArmKind,
}

/// A `ListedDecline`, its callee held as the body itself.
pub(crate) struct PendingDecline {
    pub(crate) scope: u32,
    pub(crate) program_counter: u32,
    pub(crate) callee: Option<Rc<CompiledCode>>,
    pub(crate) reason: InlineDecline,
}

impl PendingListing {
    /// The customization key the listing names, if any.
    pub(crate) fn key(&self) -> Option<CustomizationKey> {
        match self {
            PendingListing::Code(code) => code.key,
            PendingListing::Inlining(inlining) => inlining.key,
        }
    }

    /// The arms whose receivers are not named yet: those a full collection
    /// that reclaims their ids names before the flush.
    pub(crate) fn unnamed_arms(&mut self) -> impl Iterator<Item = &mut PendingArm> {
        let arms = match self {
            PendingListing::Code(_) => None,
            PendingListing::Inlining(inlining) => Some(inlining.arms.iter_mut()),
        };
        arms.into_iter().flatten().filter(|arm| arm.named.is_none())
    }

    /// The listing of `code`, named `name`, its key named `key`, the rest
    /// of its bodies and its receivers named now, as the flush names them.
    pub(crate) fn delivered(
        self,
        mutator: &Mutator<'_>,
        names: &mut BodyNames,
        code: &CompiledCode,
        name: String,
        key: Option<(String, Option<String>)>,
    ) -> Listing {
        match self {
            PendingListing::Code(pending) => {
                let root = ListedScope {
                    number: 0,
                    body: name.clone(),
                    parent: None,
                    instructions: bytecode::instruction_lines(code),
                };
                let inlined = pending.scopes.into_iter().map(
                    |PendingScope {
                         number,
                         code,
                         parent,
                     }| ListedScope {
                        number,
                        body: names.name(mutator, &code),
                        parent,
                        instructions: bytecode::instruction_lines(&code),
                    },
                );
                let scopes = std::iter::once(root).chain(inlined).collect();
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
                    scopes,
                    mark: None,
                })
            }
            PendingListing::Inlining(pending) => {
                let root = ListedScope {
                    number: 0,
                    body: name.clone(),
                    parent: None,
                    instructions: Vec::new(),
                };
                let inlined = pending.scopes.into_iter().map(
                    |PendingScope {
                         number,
                         code,
                         parent,
                     }| ListedScope {
                        number,
                        body: names.name(mutator, &code),
                        parent,
                        instructions: Vec::new(),
                    },
                );
                let scopes = std::iter::once(root).chain(inlined).collect();
                let arms = pending
                    .arms
                    .into_iter()
                    .map(|arm| ListedArm {
                        scope: arm.scope,
                        program_counter: arm.program_counter,
                        receiver: arm
                            .named
                            .unwrap_or_else(|| class_name_of_id(mutator, arm.receiver)),
                        inlined: arm.inlined,
                    })
                    .collect();
                let declined = pending
                    .declined
                    .into_iter()
                    .map(|declined| ListedDecline {
                        scope: declined.scope,
                        program_counter: declined.program_counter,
                        callee: declined.callee.map(|callee| names.name(mutator, &callee)),
                        reason: declined.reason,
                    })
                    .collect();
                Listing::Inlining(InliningListing {
                    body: name,
                    key,
                    scopes,
                    arms,
                    declined,
                    outcome: pending.outcome,
                })
            }
        }
    }
}
