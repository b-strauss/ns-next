//! `Expression`/`Statement` -> `Instruction` lowering, register allocation, and
//! the scope stack that maps resolver depths onto runtime frame hops. See
//! `docs/design/compiler.md`'s "Registers" and "Control-flow inlining"
//! sections.
//!
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use ns_syntax::ast::{self, Expression, ExpressionKind, Statement, StatementKind};
use ns_syntax::Span;

use super::capture::{BodyKey, Captured, Captures, Place};
use super::control_flow::{self, InlineForm};
use super::simultaneous;
use crate::builtins::primitive::MirrorPrimitive;
use crate::bytecode::{
    CallSite, CodeBuilder, CodeKind, CompiledCode, Instruction, IntegerOperation,
    Literal as BytecodeLiteral,
};
use crate::interpreter::literals::{parse_float_literal, parse_integer_literal, IntegerLiteral};
use crate::types::Binding;

/// One entry of the compile-time scope stack. The resolver counts every
/// block literal as one scope; at run time only `Frame` scopes cost a
/// `captured_frame` hop -- an `Inlined` scope (control-flow inlining) has
/// no frame.
///
/// `by_reference` marks a fallback closure's frame and everything nested in
/// it, the one place the optimized lowering still reaches a frame's
/// registers by reference (`docs/design/execution.md`, "Fallback blocks").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeKind {
    Frame { key: BodyKey, by_reference: bool },
    Inlined,
}

/// Where a body keeps what it can reach: its own locals, the values it was
/// handed when it was created, and its indirection vector
/// (`docs/design/execution.md`, "Converted representation").
pub(crate) struct Layout {
    key: BodyKey,
    argument_count: u16,
    /// What a block of this body's key is handed when it is created, in the
    /// order its own registers take them. Empty for a root body and for a
    /// by-reference one.
    copied: Vec<Captured>,
    /// One entry per local, by resolver index. All `Register` for a
    /// by-reference body.
    places: Vec<Place>,
    /// The hidden register holding this body's indirection vector, present
    /// only when it has vector variables.
    vector_register: Option<u16>,
    vector_size: u16,
    /// `copied.len()`, which fits a `u16` because `base` does.
    copied_count: u16,
    /// One past this body's last named register: the parameters, the copied
    /// values, the temporaries and the vector register. Everything above is
    /// scratch. A layout exists only when this fits a `u16`, which is what
    /// keeps every named register's index below from overflowing.
    base: u16,
}

/// Where one frame keeps a variable, in that frame's own registers -- the
/// answer before the reading body's own distance from that frame is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Location {
    Register(u16),
    Vector { vector_register: u16, slot: u16 },
}

/// How the body being lowered reaches one variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    Register(u16),
    OuterRegister { hops: u16, index: u16 },
    Vector { vector: VectorAccess, slot: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VectorAccess {
    Register(u16),
    Outer { hops: u16, index: u16 },
}

/// What a nested `Lowerer` inherits from the one that creates it: the scope
/// stack, and one entry per enclosing `Frame` scope (innermost last) of the
/// two tables a write or a read through a frame hop consults.
#[derive(Default)]
pub(crate) struct Enclosing {
    pub(crate) scopes: Vec<ScopeKind>,
    pub(crate) read_only_locals: Vec<Rc<[bool]>>,
    pub(crate) layouts: Vec<Rc<Layout>>,
}

impl Layout {
    /// The layout of a converted body: the copied values sit between the
    /// parameters and the temporaries, and one hidden register after the
    /// temporaries holds the indirection vector when the body has one.
    /// `None` when its named registers do not fit a `u16`: the body is past
    /// the register ceiling.
    fn converted(
        key: BodyKey,
        argument_count: u16,
        temporary_count: u16,
        captures: &Captures,
    ) -> Option<Self> {
        let body = captures.body(key);
        let copied = body.copied.clone();
        let copied_count = u16::try_from(copied.len()).ok()?;
        let below_vector = argument_count
            .checked_add(copied_count)?
            .checked_add(temporary_count)?;
        let (vector_register, base) = if body.vector_size > 0 {
            (Some(below_vector), below_vector.checked_add(1)?)
        } else {
            (None, below_vector)
        };
        Some(Layout {
            key,
            argument_count,
            copied,
            places: body.places.clone(),
            vector_register,
            vector_size: body.vector_size,
            copied_count,
            base,
        })
    }

    /// The layout of a fallback closure, or of a block nested in one: no
    /// copied values and no vector, so every local sits in the register its
    /// resolver index names (`docs/design/execution.md`, "Fallback blocks").
    /// `None` when its named locals do not fit a `u16`.
    fn by_reference(key: BodyKey, argument_count: u16, temporary_count: u16) -> Option<Self> {
        let base = argument_count.checked_add(temporary_count)?;
        Some(Layout {
            key,
            argument_count,
            copied: Vec::new(),
            places: vec![Place::Register; usize::from(base)],
            vector_register: None,
            vector_size: 0,
            copied_count: 0,
            base,
        })
    }

    /// The layout a body past the register ceiling is given, since it is
    /// never laid out: no register at all. Nothing is lowered against it
    /// (`Lowerer::too_large`).
    fn past_the_ceiling(key: BodyKey) -> Self {
        Layout {
            key,
            argument_count: 0,
            copied: Vec::new(),
            places: Vec::new(),
            vector_register: None,
            vector_size: 0,
            copied_count: 0,
            base: 0,
        }
    }

    /// The register a local of *this* body sits in: the copied values sit
    /// between the parameters and the temporaries, so every temporary is
    /// pushed up by their count. Below `base`, so it fits.
    fn local_register(&self, index: u16) -> u16 {
        if index < self.argument_count {
            index
        } else {
            index
                .checked_add(self.copied_count)
                .expect("internal: a local's register past the body's base")
        }
    }

    /// The register of a frame with this layout that holds `entry` -- its own
    /// local or vector when `entry` names this body, and otherwise the copied
    /// register it was handed when it was created.
    fn register_holding(&self, entry: Captured) -> u16 {
        match entry {
            Captured::Variable { body, index } if body == self.key => self.local_register(index),
            Captured::Vector { body } if body == self.key => self
                .vector_register
                .expect("internal: a vector variable in a body with no vector"),
            _ => {
                let position =
                    self.copied.iter().position(|held| *held == entry).expect(
                        "internal: a variable the capture analysis did not hand this block",
                    );
                super::local_index(usize::from(self.argument_count) + position)
            }
        }
    }

    /// Where a frame with this layout keeps the variable `body` declares at
    /// `index`.
    fn locate(&self, body: BodyKey, index: u16, captures: &Captures) -> Location {
        match captures.body(body).places[index as usize] {
            Place::Register => {
                Location::Register(self.register_holding(Captured::Variable { body, index }))
            }
            Place::Vector(slot) => Location::Vector {
                vector_register: self.register_holding(Captured::Vector { body }),
                slot,
            },
        }
    }
}

/// `location`, read from a body `hops` frames below the one that holds it.
fn access_through(hops: u16, location: Location) -> Access {
    match location {
        Location::Register(register) if hops == 0 => Access::Register(register),
        Location::Register(register) => Access::OuterRegister {
            hops,
            index: register,
        },
        Location::Vector {
            vector_register,
            slot,
        } if hops == 0 => Access::Vector {
            vector: VectorAccess::Register(vector_register),
            slot,
        },
        Location::Vector {
            vector_register,
            slot,
        } => Access::Vector {
            vector: VectorAccess::Outer {
                hops,
                index: vector_register,
            },
            slot,
        },
    }
}

/// The runtime `captured_frame` hop count from the innermost scope up to the
/// scope at `position`: the number of `Frame` scopes above it.
fn hops_above(scopes: &[ScopeKind], position: usize) -> u16 {
    let hops = scopes[position + 1..]
        .iter()
        .filter(|scope| matches!(scope, ScopeKind::Frame { .. }))
        .count();
    // One scope per block literal, and the parser refuses nesting past its
    // `MAX_DEPTH` of 128, so a hop count never nears `u16::MAX`.
    u16::try_from(hops).expect("internal: more block nesting than the parser admits")
}

/// Whether anything reads an expression's value once it is computed. A position
/// states it, never infers it; `Lowerer::lower_expression_for` is the one place
/// that acts on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueUse {
    /// An instruction reads the value: a return, `InitializeSlot`, a body's
    /// `ImplicitReturn`, an inlined block's result.
    Read,
    /// Nothing reads the value: every statement but a block's last, every
    /// statement of a method or an initializer (both fall off their end
    /// answering `self`), an inlined loop body's statements, and the last
    /// statement of a block inlined where the inlined form's own value is
    /// discarded.
    Discarded,
}

/// The position in `scopes` (innermost last) of the scope a resolver depth
/// of `depth` names, and the `Frame` scope it must be.
fn declaring_scope(scopes: &[ScopeKind], depth: u16) -> (usize, BodyKey, bool) {
    let position = scopes.len() - 1 - depth as usize;
    match scopes[position] {
        ScopeKind::Frame { key, by_reference } => (position, key, by_reference),
        ScopeKind::Inlined => unreachable!(
            "internal: a local declared by an inlined block (the inlining gate admits no names there)"
        ),
    }
}

pub(crate) struct Lowerer<'a> {
    bindings: &'a Rc<HashMap<Span, Binding>>,
    kind: CodeKind,
    /// Saturated at `u16::MAX`: only a body that is `too_large` meets a
    /// count past it.
    argument_count: u16,
    /// Whether this body needs more registers than a `u16` numbers: its
    /// named registers do not fit (it is never laid out, and lowers
    /// nothing), or its scratch registers ran past `u16::MAX`
    /// (`allocate_scratch`). From then on nothing emitted is kept: `finish`
    /// answers `super::body_too_large`, never a body whose register indices
    /// wrapped.
    too_large: bool,
    /// The capture analysis of the whole code family, shared by every body
    /// of it (`compiler/capture.rs`).
    captures: Rc<Captures>,
    /// This body's own layout.
    layout: Rc<Layout>,
    /// The layout of each enclosing `Frame` scope, one entry per `Frame` in
    /// `scopes` other than this body's own, innermost enclosing frame last --
    /// parallel to `enclosing_read_only_locals`.
    layouts: Vec<Rc<Layout>>,
    /// Whether this body is a fallback closure, or a block nested in one, and
    /// so reaches an enclosing frame's registers by reference
    /// (`docs/design/execution.md`, "Fallback blocks").
    by_reference: bool,
    /// Whether the block literals lowered right now become fallback closures:
    /// set while `lower_fallback_send` lowers an inlined form's guard-failure
    /// arm out of the very block literals the common path inlined.
    blocks_by_reference: bool,
    /// Innermost last. The last entry is this body's own `Frame` scope
    /// with `Inlined` entries pushed above it while an inlined block body
    /// is being lowered.
    scopes: Vec<ScopeKind>,
    code: CodeBuilder,
    base: u16,
    next_scratch: u16,
    max_register: u16,
    statement_index: usize,
    statement_program_counter: Vec<(usize, Range<usize>)>,
    /// The next send's number within this family (`take_call_site`).
    next_call_site: u32,
    /// The most recent top-level `Statement::Expression`'s value register, for
    /// `finish_body`'s block case and for reading an inlined
    /// block's result.
    last_expression_register: Option<u16>,
    /// For each named local's register (`< base`), whether nothing but its
    /// initialization can write it: every parameter, and every immutable
    /// local slot (§3.6). The resolver binds no assignment to one
    /// (`docs/design/dispatch.md`, "Setter sends"), so an operand read from
    /// one survives anything lowered after it. `Rc<[bool]>` so
    /// `lower_block_literal` can hand a nested `Lowerer` a copy of it (as
    /// one entry of that nested `Lowerer`'s own `enclosing_read_only_locals`)
    /// without cloning the table's contents.
    read_only_locals: Rc<[bool]>,
    /// The `read_only_locals` table of each enclosing `Frame` scope, one
    /// entry per `Frame` in `scopes` other than this body's own, innermost
    /// enclosing frame last. Lets `lower_assign`'s `StoreOuterLocal` branch
    /// (`hops != 0`) check the same precondition the `StoreLocal` branches
    /// already `debug_assert!` for `hops == 0` -- a write from a nested
    /// block into an *enclosing* frame's read-only local, the write-through-a-
    /// closure case `materialize`'s copy exists to protect against
    /// (`CLAUDE.md`, "An expression's value register must still hold that
    /// value").
    enclosing_read_only_locals: Vec<Rc<[bool]>>,
    /// For each named local's register (`< base`), whether a fallback closure
    /// can write it at any later time -- the one way user code a send runs
    /// reaches a register of this activation after closure conversion
    /// (`docs/design/execution.md`, "Fewer protective copies"). In a
    /// converted body that is exactly the locals an inlinable conditional of
    /// this body assigns (`BodyCaptures::branch_assigned`), because the
    /// fallback closure that conditional builds out of the same block literal
    /// writes them by reference; in a by-reference body it is every writable
    /// register, since every closure nested in one reaches this frame by
    /// reference too.
    reachable_by_a_fallback_closure: Vec<bool>,
}

impl<'a> Lowerer<'a> {
    /// The nine parameters are the body's identity (`kind`, `key`), its own
    /// shape (`argument_count`, `temporaries`), the two analyses every body
    /// of the family shares (`bindings`, `captures`), what makes it a
    /// fallback closure (`by_reference`), what it inherits from its creator
    /// (`enclosing`) and where its sends' numbering resumes.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        kind: CodeKind,
        key: BodyKey,
        argument_count: usize,
        temporaries: &[ast::SlotDeclaration],
        bindings: &'a Rc<HashMap<Span, Binding>>,
        captures: Rc<Captures>,
        by_reference: bool,
        enclosing: Enclosing,
        first_call_site: u32,
    ) -> Self {
        let Enclosing {
            mut scopes,
            read_only_locals: enclosing_read_only_locals,
            layouts,
        } = enclosing;
        scopes.push(ScopeKind::Frame { key, by_reference });
        // Judged before the capture analysis is asked for the body: it
        // declares no body whose named locals do not fit, and the resolver's
        // indices for its locals past `u16::MAX` name nothing.
        let layout = if super::named_locals_fit(argument_count, temporaries.len()) {
            let argument_count = super::local_index(argument_count);
            let temporary_count = super::local_index(temporaries.len());
            if by_reference {
                Layout::by_reference(key, argument_count, temporary_count)
            } else {
                Layout::converted(key, argument_count, temporary_count, &captures)
            }
        } else {
            None
        };
        let too_large = layout.is_none();
        let argument_count = super::saturated_count(argument_count);
        let layout = layout.unwrap_or_else(|| Layout::past_the_ceiling(key));
        // A body past the ceiling has no register table to fill.
        let named_temporaries = if too_large { &[][..] } else { temporaries };
        let base = layout.base;
        // Indexed by *register*, not by resolver index. A parameter and an
        // immutable local slot are read-only because nothing but their
        // initialization writes them (§3.6); a copied register because the
        // analysis puts every variable anything writes after capture into a
        // vector; the vector register because only `MakeVector` writes it;
        // and a vector variable's own register because it stays nil. A
        // simultaneous temporary (§6.3.2) is written twice whatever its
        // declaration says: its future, then its value at the clause's end.
        let mut read_only_locals = vec![true; usize::from(base)];
        for (offset, temporary) in named_temporaries.iter().enumerate() {
            let index = super::temporary_index(argument_count, offset);
            let writable = (temporary.mutability != ast::SlotMutability::Immutable
                || temporary.simultaneous)
                && layout.places[index as usize] == Place::Register;
            read_only_locals[layout.local_register(index) as usize] = !writable;
        }
        // Also indexed by register. A by-reference body has no capture
        // analysis of its own -- it was walked as the inlined block it also
        // is -- and anything nested in it reaches every one of its registers,
        // so every writable register counts.
        let mut reachable_by_a_fallback_closure = vec![false; usize::from(base)];
        if by_reference {
            for (register, read_only) in read_only_locals.iter().enumerate() {
                reachable_by_a_fallback_closure[register] = !read_only;
            }
        } else if !too_large {
            // A body past the ceiling was never analysed, and has no
            // register to mark.
            let body = captures.body(key);
            for (index, assigned) in body.branch_assigned.iter().enumerate() {
                // A vector variable's own register stays nil: the fallback
                // closure writes the vector slot instead.
                if *assigned && body.places[index] == Place::Register {
                    let register = layout.local_register(super::local_index(index));
                    reachable_by_a_fallback_closure[register as usize] = true;
                }
            }
        }
        let mut code = CodeBuilder::new();
        // Before the local initializers, which already write through it:
        // `| x ::= 3. |` on a vector variable is a `StoreVector`.
        if let Some(destination) = layout.vector_register {
            code.emit(Instruction::MakeVector {
                destination,
                size: layout.vector_size,
            });
        }
        Lowerer {
            bindings,
            kind,
            argument_count,
            too_large,
            captures,
            layout: Rc::new(layout),
            layouts,
            by_reference,
            blocks_by_reference: false,
            scopes,
            code,
            base,
            next_scratch: base,
            max_register: base,
            statement_index: 0,
            statement_program_counter: Vec::new(),
            next_call_site: first_call_site,
            last_expression_register: None,
            read_only_locals: read_only_locals.into(),
            enclosing_read_only_locals,
            reachable_by_a_fallback_closure,
        }
    }

    /// A scratch register. Checked in every build: past `u16::MAX` it
    /// marks the body too large and answers the last register again, which
    /// `finish` never keeps -- a wrapped index would be silently wrong code.
    fn allocate_scratch(&mut self) -> u16 {
        let register = self.next_scratch;
        match register.checked_add(1) {
            Some(next) => {
                self.next_scratch = next;
                self.max_register = self.max_register.max(next);
            }
            None => self.too_large = true,
        }
        register
    }

    fn emit(&mut self, instruction: Instruction) -> usize {
        self.code.emit(instruction)
    }

    /// Lowers a body's own statements, then its end (`finish_body`). The
    /// last statement's value is read only when falling off the end answers
    /// it; every other statement's value is discarded.
    pub(crate) fn lower_body(&mut self, statements: &[Statement]) {
        if self.too_large {
            return;
        }
        let last_use = if self.answers_last_value() {
            ValueUse::Read
        } else {
            ValueUse::Discarded
        };
        for (index, statement) in statements.iter().enumerate() {
            if self.too_large {
                // Nothing more is kept (`finish`): stop lowering.
                return;
            }
            let use_ = if index + 1 == statements.len() {
                last_use
            } else {
                ValueUse::Discarded
            };
            self.lower_statement(statement, use_);
        }
        self.finish_body(statements);
    }

    /// Lowers one of the body's own statements, recording its safepoint range
    /// in `statement_program_counter` and resetting the scratch allocator first
    /// (statements are the safepoint/scratch-liveness boundary).
    fn lower_statement(&mut self, statement: &Statement, use_: ValueUse) {
        self.next_scratch = self.base;
        let start = self.code.len();
        self.lower_statement_inner(statement, use_);
        let end = self.code.len();
        self.statement_program_counter
            .push((self.statement_index, start..end));
        self.statement_index += 1;
    }

    /// An initializer's clauses (`docs/design/object-model.md`,
    /// "Instantiation"), each run on `self` at the level the initializer was
    /// entered at, as the levels between it and the superclass clause's
    /// class stand above it: first §6.1's check of every `<:` clause against
    /// the class it names, the nearest level first (the last clause, one
    /// step up) and the topmost last, before anything runs; then
    /// the superclass clause's message's arguments and the superclass's
    /// initializer with them (§6.3); then, from the lowest level up, each
    /// `<:` clause's message's arguments and its level's own part with them
    /// (§6.2). Every argument is evaluated in the factory's scope, where the
    /// resolver bound the factory's parameters for it.
    ///
    /// `has_body` says whether the declaration adds a level of its own
    /// above the levels its `<:` clauses add, which the levels are then
    /// counted from: the level `<:` clause `k` of `n` adds is `n - k + 1`
    /// steps up, the superclass clause's class `n + 1`. A body-less
    /// declaration's class is the level its last clause adds, one step
    /// lower. A chain of more levels than a `u16` counts leaves the body
    /// too large.
    ///
    /// One statement for the checks, one for the superclass and one for
    /// each applied level, each with its own scratch registers: nothing is
    /// live across them.
    pub(crate) fn lower_instance_initialization_prologue(
        &mut self,
        superclass: Option<&ast::SuperclassClause>,
        applied: &[ast::SuperclassClause],
        has_body: bool,
    ) {
        if self.too_large {
            return;
        }
        let own_level = usize::from(has_body);
        let Some(superclass_steps) = applied
            .len()
            .checked_add(own_level)
            .and_then(|steps| u16::try_from(steps).ok())
        else {
            self.too_large = true;
            return;
        };
        // The level clause `position` (from 0) adds; below the superclass
        // clause's, so it fits a `u16` too.
        let steps_of = |position: usize| superclass_steps - 1 - position as u16;
        if !applied.is_empty() {
            let start = self.code.len();
            for (position, clause) in applied.iter().enumerate().rev() {
                self.emit(Instruction::CheckAppliedClause {
                    steps: steps_of(position),
                    selector: super::clause_selector(Some(clause)),
                });
            }
            self.record_statement(start);
        }
        self.next_scratch = self.base;
        let start = self.code.len();
        let (argument_base, argument_sources) =
            self.lower_arguments(&[], super::clause_arguments(superclass));
        let destination = self.allocate_scratch();
        self.emit(Instruction::InitializeSuperclass {
            steps: superclass_steps,
            selector: super::clause_selector(superclass),
            argument_base,
            argument_sources,
            destination,
        });
        self.record_statement(start);
        for (position, clause) in applied.iter().enumerate() {
            if self.too_large {
                // Nothing more is kept (`finish`): stop lowering.
                return;
            }
            self.next_scratch = self.base;
            let start = self.code.len();
            let (argument_base, argument_sources) =
                self.lower_arguments(&[], super::clause_arguments(Some(clause)));
            let destination = self.allocate_scratch();
            self.emit(Instruction::InitializeAppliedLevel {
                steps: steps_of(position),
                argument_base,
                argument_sources,
                destination,
            });
            self.record_statement(start);
        }
    }

    /// The initializer's per-slot statement: the slot's initializer expression,
    /// then `InitializeSlot`, which writes the field directly, never through a
    /// setter send. One `statement_program_counter` entry. `own_index` is the
    /// slot's position among the mixin's own slots (declaration order) -- the
    /// VM turns it into an absolute field index at run time via the executing
    /// activation's slot base (`Heap::activation_slot_base`).
    pub(crate) fn lower_slot_initializer(&mut self, initializer: &Expression, own_index: u32) {
        if self.too_large {
            return;
        }
        self.next_scratch = self.base;
        let start = self.code.len();
        let source = self.lower_expression_for(initializer, ValueUse::Read);
        self.emit(Instruction::InitializeSlot { source, own_index });
        self.record_statement(start);
    }

    /// A simultaneous slot's first write (§6.3.2): `block`, its initializer's
    /// future block (`simultaneous::future_block`), made into a closure and
    /// then a future, stored with `InitializeSlotProvisionally`, which skips
    /// a value's slot check -- the clause's end stores the checked value
    /// (`lower_simultaneous_slot_end`). One statement.
    pub(crate) fn lower_simultaneous_slot_initializer(
        &mut self,
        block: &Expression,
        own_index: u32,
    ) {
        if self.too_large {
            return;
        }
        self.next_scratch = self.base;
        let start = self.code.len();
        let closure = self.lower_expression_for(block, ValueUse::Read);
        let future = self.allocate_scratch();
        self.emit(Instruction::MakeFuture {
            destination: future,
            closure,
        });
        self.emit(Instruction::InitializeSlotProvisionally {
            source: future,
            own_index,
        });
        self.record_statement(start);
    }

    /// A simultaneous slot's second write, at its clause's end (§6.3.2):
    /// the slot read back without a getter send, which a subclass could
    /// override, resolved, and stored with an ordinary `InitializeSlot`.
    /// `ResolveFuture` runs user code -- the future's block -- exactly as a
    /// send does, so it counts as one for the value-register invariant; the
    /// only register live across it is this statement's own scratch
    /// register, which no send can write. One statement.
    pub(crate) fn lower_simultaneous_slot_end(&mut self, own_index: u32) {
        if self.too_large {
            return;
        }
        self.next_scratch = self.base;
        let start = self.code.len();
        let register = self.allocate_scratch();
        self.emit(Instruction::LoadOwnSlot {
            destination: register,
            own_index,
        });
        self.emit(Instruction::ResolveFuture {
            destination: register,
            source: register,
        });
        self.emit(Instruction::InitializeSlot {
            source: register,
            own_index,
        });
        self.record_statement(start);
    }

    /// Records the statement that began at instruction `start` and ends
    /// here, and moves on to the next statement index.
    fn record_statement(&mut self, start: usize) {
        let end = self.code.len();
        self.statement_program_counter
            .push((self.statement_index, start..end));
        self.statement_index += 1;
    }

    /// A lazy slot's getter (§6.3.3): `ReturnSlotIfFilled`, which answers
    /// the slot once it holds anything but `nil`, then the initializer, then
    /// `InitializeSlot` -- "the slot's value is set to o (but not by means of
    /// a setter method)" -- then `Return` of the same register, "and o is
    /// returned as the result of the send". Nothing runs between the two
    /// reads of the register, so it needs no protective copy. The
    /// activation's slot base is that of the application it was entered at
    /// (`Heap::activation_slot_base`), the class whose slot this is. The
    /// test precedes the one statement, as a prologue.
    pub(crate) fn lower_lazy_slot_getter(&mut self, initializer: &Expression, own_index: u32) {
        if self.too_large {
            return;
        }
        self.next_scratch = self.base;
        self.emit(Instruction::ReturnSlotIfFilled { own_index });
        let start = self.code.len();
        let source = self.lower_expression_for(initializer, ValueUse::Read);
        self.emit(Instruction::InitializeSlot { source, own_index });
        self.emit(Instruction::Return { register: source });
        let end = self.code.len();
        self.statement_program_counter
            .push((self.statement_index, start..end));
        self.statement_index += 1;
    }

    /// Each local slot with an initializer, in declaration order, as one
    /// statement: its expression, then a `StoreLocal` into the slot's own
    /// register, which follows the parameters (§3.6). These are the body's
    /// first statements, so an activation's statement indices run through
    /// its local initializers before its statements, the way an instance
    /// initializer's run through its slot initializers before its
    /// `initExprs`. They leave `last_expression_register` alone: a body's
    /// value is its last *statement's*, and a body with none answers nil.
    ///
    /// A simultaneous clause (§6.3.2) stores each initializer's future
    /// instead -- `MakeFuture` of its future block
    /// (`simultaneous::future_block`) -- and then, at the clause's end, reads
    /// each of those temporaries back and stores it resolved, one statement
    /// apiece, in declaration order.
    pub(crate) fn lower_local_initializers(&mut self, temporaries: &[ast::SlotDeclaration]) {
        if self.too_large {
            return;
        }
        for (position, temporary) in temporaries.iter().enumerate() {
            if self.too_large {
                // Nothing more is kept (`finish`): stop lowering.
                return;
            }
            let Some(initializer) = &temporary.initializer else {
                continue;
            };
            self.next_scratch = self.base;
            let start = self.code.len();
            let source = match simultaneous::future_block(temporary) {
                Some(block) => {
                    let closure = self.lower_expression_for(&block, ValueUse::Read);
                    let future = self.allocate_scratch();
                    self.emit(Instruction::MakeFuture {
                        destination: future,
                        closure,
                    });
                    future
                }
                None => self.lower_expression_for(initializer, ValueUse::Read),
            };
            // Through the same `Access` as an assignment: a captured-and-
            // written local's initializer writes its scope's vector, not a
            // register (`docs/design/execution.md`, "Converted representation").
            let access =
                self.resolve_variable(0, super::temporary_index(self.argument_count, position));
            self.emit_store(access, source);
            self.record_statement(start);
        }
        for (position, temporary) in temporaries.iter().enumerate() {
            if self.too_large {
                // Nothing more is kept (`finish`): stop lowering.
                return;
            }
            if !temporary.simultaneous || temporary.initializer.is_none() {
                continue;
            }
            self.next_scratch = self.base;
            let start = self.code.len();
            let access =
                self.resolve_variable(0, super::temporary_index(self.argument_count, position));
            let read = self.emit_read(access, None);
            // `ResolveFuture` runs user code -- the future's block -- exactly
            // as a send does, and runs again once that block has answered
            // (`AfterForcing::Reexecute`), reading its source afresh. So its
            // source is protected as an operand across a send is: copied out
            // of a named local's register that a send can write.
            let source = self.materialize(read, self.a_send_can_write(read));
            let resolved = self.allocate_scratch();
            self.emit(Instruction::ResolveFuture {
                destination: resolved,
                source,
            });
            self.check_writable(access);
            self.emit_store(access, resolved);
            self.record_statement(start);
        }
    }

    /// Lowers a statement without touching the allocator or the PC table --
    /// shared by `lower_statement` and `lower_inline_block_body`. `use_` is
    /// what reads an expression statement's value; a `^` that returns reads
    /// its own. Returns the value register for a `Statement::Expression` (or a
    /// discarded initializer `^`), `None` when a return instruction was
    /// emitted.
    fn lower_statement_inner(&mut self, statement: &Statement, use_: ValueUse) -> Option<u16> {
        match &statement.kind {
            StatementKind::Expression(expression) => {
                let register = self.lower_expression_for(expression, use_);
                self.last_expression_register = Some(register);
                Some(register)
            }
            StatementKind::Return(expression) => {
                let make_return = self.return_instruction();
                let use_ = if make_return.is_some() {
                    ValueUse::Read
                } else {
                    use_
                };
                let register = self.lower_expression_for(expression, use_);
                match make_return {
                    Some(make_return) => {
                        self.emit(make_return(register));
                        self.last_expression_register = None;
                        None
                    }
                    None => {
                        self.last_expression_register = Some(register);
                        Some(register)
                    }
                }
            }
        }
    }

    /// What a `^` means here, per code kind: the instruction that returns a
    /// value register. `None` is "evaluate and discard" (a top-level `^` in
    /// `initExprs`, which has no method to return from). Inside an inlined
    /// block (`scopes.last() == Inlined`) an initializer's `^` must instead
    /// raise the same `Signal::Return` a real block would, so it becomes
    /// `NonLocalReturn`.
    fn return_instruction(&self) -> Option<fn(u16) -> Instruction> {
        let inside_inlined = self.scopes.last() == Some(&ScopeKind::Inlined);
        match (self.kind, inside_inlined) {
            (CodeKind::Method, _) => Some(|register| Instruction::Return { register }),
            (CodeKind::Block, _) => Some(|register| Instruction::NonLocalReturn { register }),
            (CodeKind::Initializer, false) => None,
            (CodeKind::Initializer, true) => {
                Some(|register| Instruction::NonLocalReturn { register })
            }
            // The clause body is a single synthetic `^expression` (the
            // receiver), so its top-level `^` is what hands that value
            // back; a `^` written inside a block of the receiver returns
            // from the clause's own activation, which catches it
            // (`Mutator::catches_return`), exactly as a method's activation
            // would.
            (CodeKind::SuperclassClause, false) => {
                Some(|register| Instruction::Return { register })
            }
            (CodeKind::SuperclassClause, true) => {
                Some(|register| Instruction::NonLocalReturn { register })
            }
        }
    }

    /// Whether a body that falls off its end answers its last statement's
    /// value -- a block, a superclass clause -- rather than `self` -- a
    /// method, an initializer. `finish_body` emits that end, and
    /// `lower_body` reads it to know whether the last statement's value is
    /// read, so the two cannot disagree.
    fn answers_last_value(&self) -> bool {
        match self.kind {
            CodeKind::Block | CodeKind::SuperclassClause => true,
            CodeKind::Method | CodeKind::Initializer => false,
        }
    }

    fn emit_nil(&mut self) -> u16 {
        let destination = self.allocate_scratch();
        self.emit(Instruction::LoadLiteral {
            destination,
            literal: BytecodeLiteral::Nil,
        });
        destination
    }

    /// Appends the body's terminating instruction if `statements` (the body
    /// just lowered) did not already end in one, so `vm::run` never has to
    /// handle "fell off the end". A method returns `self`; an initializer
    /// always returns `self` (its top-level `^` never emits a return); a block
    /// returns its last statement's value, `nil` if it has none; a superclass
    /// clause returns the value of the one expression it is.
    fn finish_body(&mut self, statements: &[Statement]) {
        let ends_in_return = matches!(
            statements.last().map(|statement| &statement.kind),
            Some(StatementKind::Return(_))
        ) && self.kind != CodeKind::Initializer;
        if ends_in_return {
            return;
        }
        let register = if self.answers_last_value() {
            match self.last_expression_register {
                Some(last_register) => last_register,
                None => self.emit_nil(),
            }
        } else {
            let destination = self.allocate_scratch();
            self.emit(Instruction::LoadSelf { destination });
            destination
        };
        self.emit(Instruction::ImplicitReturn { register });
    }

    /// Compiles a block literal into its own `CompiledCode` with a nested
    /// `Lowerer` that inherits this one's whole scope stack, and emits
    /// `MakeClosure`. `captured_frame`/`home`/`bindings` are supplied by the
    /// VM at run time (`docs/design/execution.md`, "Activations").
    fn lower_block_literal(&mut self, block: &ast::BlockExpression, into: Option<u16>) -> u16 {
        let parameters: Vec<crate::Symbol> = block
            .parameters
            .iter()
            .map(|parameter| crate::Symbol::intern(&parameter.name.name))
            .collect();
        let key = BodyKey::Block(block.span);
        // A fallback closure's block, and everything nested in one, keeps the
        // by-reference model; the capture analysis holds no entry for such a
        // body, because it walked that block literal as the inlined one it
        // also is (`docs/design/execution.md`, "Fallback blocks").
        let by_reference = self.by_reference || self.blocks_by_reference;
        let first_call_site = self.next_call_site;
        let mut enclosing_read_only_locals = self.enclosing_read_only_locals.clone();
        enclosing_read_only_locals.push(self.read_only_locals.clone());
        let mut layouts = self.layouts.clone();
        layouts.push(self.layout.clone());
        let mut inner = Lowerer::new(
            CodeKind::Block,
            key,
            parameters.len(),
            &block.body.temporaries,
            self.bindings,
            self.captures.clone(),
            by_reference,
            Enclosing {
                scopes: self.scopes.clone(),
                read_only_locals: enclosing_read_only_locals,
                layouts,
            },
            self.next_call_site,
        );
        inner.lower_local_initializers(&block.body.temporaries);
        inner.lower_body(&block.body.statements);
        // The block's sends share the enclosing family's one counter, so the
        // root and every nested block number densely across the whole
        // family, never restarting at each `MakeClosure`
        // (`docs/design/dispatch.md`, "Code families").
        let after = inner.next_call_site;
        let too_large = inner.too_large;
        let code = inner.finish(block.span);
        // A block past the register ceiling is `super::body_too_large`,
        // which takes no copied values and makes no send, so its creator
        // hands it none -- the capture analysis may not even hold an entry
        // for it -- and the family's numbering resumes where it stood, as
        // if the block made no send.
        let copied: Vec<u16> = if by_reference || too_large {
            Vec::new()
        } else {
            self.captures
                .body(key)
                .copied
                .iter()
                .map(|entry| self.layout.register_holding(*entry))
                .collect()
        };
        self.next_call_site = if too_large { first_call_site } else { after };
        let parameters = super::parameters_of(parameters, &code);
        let destination = self.target_register(into);
        self.emit(Instruction::MakeClosure {
            destination,
            parameters: Rc::new(parameters),
            code: Rc::new(code),
            copied,
        });
        destination
    }

    /// Copies `register` into a fresh scratch register when it is one of this
    /// frame's *named*, writable locals and `clobbered` says something lowered
    /// between here and the consuming instruction can write it.
    ///
    /// `lower_expression`'s contract is that the register it returns still
    /// holds that value when its consumer executes -- every consumer defers the
    /// read to instruction-execution time. A named local's register (`< base`)
    /// is live storage, so an operand that has to survive a write to it is
    /// copied out first. A register `>= base` needs no copy: no `Binding` can
    /// name it, so only this body's own instructions ever write it, and the
    /// scratch allocator never reuses a live one. Nor does a read-only local's
    /// (`read_only_locals`): a parameter or an immutable local, which nothing
    /// lowered afterwards can write, because the resolver binds no assignment
    /// to one (§3.6).
    fn materialize(&mut self, register: u16, clobbered: bool) -> u16 {
        if !self.needs_copy(register, clobbered) {
            return register;
        }
        let destination = self.allocate_scratch();
        self.emit(Instruction::Move {
            destination,
            source: register,
        });
        destination
    }

    /// `materialize`, the copy going into `slot`, a send's argument slot,
    /// rather than into a fresh scratch register: the slot is then the
    /// argument's source, and the send copies nothing.
    fn materialize_into(&mut self, register: u16, clobbered: bool, slot: u16) -> u16 {
        if !self.needs_copy(register, clobbered) {
            return register;
        }
        self.emit(Instruction::Move {
            destination: slot,
            source: register,
        });
        slot
    }

    /// Whether `materialize` copies `register`: a writable named local
    /// something lowered before its consumer can write.
    fn needs_copy(&self, register: u16, clobbered: bool) -> bool {
        clobbered && register < self.base && !self.read_only_locals[register as usize]
    }

    /// Lowers one operand of a multi-operand instruction. `rest` is
    /// everything lowered after this operand but before that instruction
    /// executes -- the arguments still to come, for instance.
    fn lower_operand(&mut self, expression: &Expression, rest: &[Expression]) -> u16 {
        let register = self.lower_expression(expression);
        let clobbered = self.clobbered_by(register, rest);
        self.materialize(register, clobbered)
    }

    /// Whether anything in `rest` can write `register`. Only a writable
    /// named local can be clobbered (`materialize`), so nothing else walks
    /// `rest`: a long argument list or tuple of one parameter would
    /// otherwise cost a walk per operand, quadratic.
    fn clobbered_by(&self, register: u16, rest: &[Expression]) -> bool {
        let writable_local = register < self.base && !self.read_only_locals[usize::from(register)];
        writable_local
            && rest
                .iter()
                .any(|later| self.writes_register(later, register, 0))
    }

    /// Lowers `expression` in a position whose caller states whether anything
    /// reads its value.
    ///
    /// This is the other half of `lower_operand`'s rule, and the one place
    /// a copy is skipped because a value is discarded. An assignment through
    /// a setter would otherwise copy a named local's register before handing
    /// it on as its value: the setter send runs user code, which can write
    /// that local through a closure that captured this frame, and it runs
    /// before any consumer reads the value -- even a consumer that is the
    /// very next instruction (`^x:: y`, with a setter that assigns `y`).
    /// Only when nothing reads the value at all is the copy pure overhead.
    ///
    /// Every position that knows its use routes through here and states it:
    /// a body's statements (`lower_body`), a `^`, a slot initializer, an
    /// inlined block's statements, and the inlined forms that pass their own
    /// use on to their blocks. None of them infers it, so a new position
    /// cannot silently be given the cheap form without the property holding.
    fn lower_expression_for(&mut self, expression: &Expression, use_: ValueUse) -> u16 {
        match (use_, &expression.kind) {
            (ValueUse::Discarded, ExpressionKind::Assign { target, value, .. }) => {
                self.lower_assign(target, value, ValueUse::Discarded)
            }
            (ValueUse::Discarded, ExpressionKind::Send(send)) => {
                self.lower_send(send, ValueUse::Discarded, None)
            }
            _ => self.lower_expression(expression),
        }
    }

    /// Whether the user code a send runs can write `register`.
    ///
    /// After closure conversion a converted closure writes no register of
    /// another activation: a captured written variable lives in its scope's
    /// indirection vector, a vector read lands in a scratch register as a
    /// snapshot, and a copied register is read-only. The one exception is a
    /// fallback closure, which writes by reference exactly the locals its
    /// inlinable branch assigns -- and such a closure can be kept and run at
    /// any later time, so any send may be the one that runs it
    /// (`docs/design/execution.md`, "Fewer protective copies").
    ///
    /// A register above `base` is unreachable by name from anywhere, so no
    /// send can write it whatever the body's shape.
    fn a_send_can_write(&self, register: u16) -> bool {
        register < self.base && self.reachable_by_a_fallback_closure[register as usize]
    }

    /// Whether reading `identifier` dispatches -- an implicit-receiver send,
    /// or a `super`-prefixed clause receiver's super send -- rather than
    /// loading a register. Mirrors `lower_identifier_read`, whose `self` case
    /// answers before the binding is consulted; the deferred
    /// unresolved-identifier case is a single register load.
    fn identifier_sends(&self, identifier: &ast::Identifier) -> bool {
        identifier.name != "self"
            && matches!(
                self.bindings.get(&identifier.span),
                Some(Binding::LexicalSend { .. } | Binding::SuperSend)
            )
    }

    /// The register of *this* body a write to `target` lands in, if any.
    ///
    /// `inlined_depth` is how many inlined-block scopes stand between this
    /// body's scope stack and `target`'s own scope: the resolver counts every
    /// block literal as a scope, and `lower_inline_block_body` pushes the
    /// matching `ScopeKind::Inlined` only while it lowers the block, which is
    /// after this question is asked.
    fn assignment_target_register(
        &self,
        target: &ast::Identifier,
        inlined_depth: u16,
    ) -> Option<u16> {
        let (depth, index) = match self.bindings.get(&target.span)? {
            Binding::Local(index) => (0, *index),
            Binding::OuterLocal { depth, index } => (*depth, *index),
            Binding::LexicalSend { .. }
            | Binding::EnclosingObject { .. }
            | Binding::SuperSend
            | Binding::MirrorSend => return None,
        };
        // The inlining gate admits no block with temporaries or parameters,
        // so a binding inside an inlined block never names that block's own
        // scope and the subtraction cannot underflow.
        let depth = depth.checked_sub(inlined_depth).unwrap_or_else(|| {
            panic!(
                "internal: an assignment inside an inlined block bound to that block's own scope"
            )
        });
        match self.resolve_variable(depth, index) {
            Access::Register(register) => Some(register),
            // Another frame's register, or a vector slot: neither is a
            // register of this body.
            Access::OuterRegister { .. } | Access::Vector { .. } => None,
        }
    }

    /// Whether lowering `expression` here can emit an instruction that writes
    /// `register`, a register of the body being lowered, before the
    /// instruction that consumes `register` executes.
    ///
    /// Conservative in one direction only: answering `true` costs one `Move`,
    /// answering `false` when a write can happen breaks the value-register
    /// invariant (`CLAUDE.md`, "An expression's value register must still hold
    /// that value when its consumer executes") -- silent wrong output, not a
    /// crash.
    fn writes_register(&self, expression: &Expression, register: u16, inlined_depth: u16) -> bool {
        match &expression.kind {
            // `LoadLiteral` and `MakeClosure` write only their own scratch
            // destination. A block literal that is not inlined runs nothing
            // here, and the body it compiles to writes its own registers.
            ExpressionKind::Literal(_) | ExpressionKind::Block(_) => false,
            ExpressionKind::Identifier(identifier) => {
                self.identifier_sends(identifier) && self.a_send_can_write(register)
            }
            ExpressionKind::Assign { target, value, .. } => {
                self.writes_register(value, register, inlined_depth)
                    || self.assignment_target_register(target, inlined_depth) == Some(register)
                    // A setter send runs user code like any other send.
                    || (self.identifier_sends(target) && self.a_send_can_write(register))
            }
            ExpressionKind::Send(send) => self.send_writes_register(send, register, inlined_depth),
            ExpressionKind::Cascade(cascade) => {
                self.a_send_can_write(register)
                    || self.writes_register(&cascade.receiver, register, inlined_depth)
                    || std::iter::once(&cascade.first)
                        .chain(cascade.rest.iter())
                        .flat_map(|message| message.arguments.iter())
                        .any(|argument| self.writes_register(argument, register, inlined_depth))
            }
            // `MakeArray` itself writes nothing but its destination; its
            // elements can.
            ExpressionKind::DynamicArray { elements, .. } => elements
                .iter()
                .any(|element| self.writes_register(element, register, inlined_depth)),
        }
    }

    /// Mirrors `lower_send` through the shared gate: only an ordinary
    /// explicit-receiver send is offered to `recognize`, and a lexical or
    /// `super` send has no receiver expression to walk. Every send runs user
    /// code, and so does every recognized inlinable form -- through the real
    /// send its guard-failure path makes (`lower_fallback_send`), or, for a
    /// loop, through the handlers the `NonBooleanReceiver` it signals runs
    /// (`emit_signal_non_boolean`).
    ///
    /// An inlined integer leaf is deliberately not named here: its fallback
    /// *is* the ordinary send, over the same registers, so it falls through
    /// to the ordinary-send answer below. Giving it a case of its own would
    /// be a second, weaker answer to the same question.
    fn send_writes_register(&self, send: &ast::Send, register: u16, inlined_depth: u16) -> bool {
        if self.a_send_can_write(register) {
            return true;
        }
        if control_flow::is_ordinary_send(self.bindings, send) {
            // The mirror's failure block is lowered into this body, like an
            // inlined conditional's blocks; its receiver and leaf arguments
            // are lowered as any send's are.
            if let Some((_, block)) = control_flow::recognize_mirror_send(self.bindings, send) {
                let arguments = &send.message.arguments;
                return self.writes_register(&send.receiver, register, inlined_depth)
                    || arguments[..arguments.len() - 1]
                        .iter()
                        .any(|argument| self.writes_register(argument, register, inlined_depth))
                    || self.inlined_blocks_write_register([block], register, inlined_depth);
            }
            if let Some(form) = control_flow::recognize(send) {
                return self.inlined_form_writes_register(send, form, register, inlined_depth);
            }
            if self.writes_register(&send.receiver, register, inlined_depth) {
                return true;
            }
        }
        send.message
            .arguments
            .iter()
            .any(|argument| self.writes_register(argument, register, inlined_depth))
    }

    /// A recognized inlinable form lowers its blocks into this same body, so
    /// an assignment in one of them writes this body's registers directly --
    /// one resolver scope deeper than the form itself.
    fn inlined_form_writes_register(
        &self,
        send: &ast::Send,
        form: InlineForm<'_>,
        register: u16,
        inlined_depth: u16,
    ) -> bool {
        let (receiver, first, second) = match form {
            InlineForm::IfTrueIfFalse(true_block, false_block) => {
                (Some(&send.receiver), Some(true_block), Some(false_block))
            }
            InlineForm::IfTrue(block)
            | InlineForm::IfFalse(block)
            | InlineForm::And(block)
            | InlineForm::Or(block) => (Some(&send.receiver), Some(block), None),
            // The receiver *is* the condition block: `lower_while` lowers no
            // receiver expression of its own.
            InlineForm::WhileTrue { condition, body }
            | InlineForm::WhileFalse { condition, body } => (None, Some(condition), Some(body)),
        };
        if let Some(receiver) = receiver {
            if self.writes_register(receiver, register, inlined_depth) {
                return true;
            }
        }
        self.inlined_blocks_write_register(
            [first, second].into_iter().flatten(),
            register,
            inlined_depth,
        )
    }

    /// Whether a statement of one of `blocks`, each lowered inline into this
    /// body, can write `register`: asked one resolver scope deeper than the
    /// send that inlines them.
    fn inlined_blocks_write_register<'b>(
        &self,
        blocks: impl IntoIterator<Item = &'b ast::BlockExpression>,
        register: u16,
        inlined_depth: u16,
    ) -> bool {
        blocks
            .into_iter()
            .flat_map(|block| block.body.statements.iter())
            .any(|statement| match &statement.kind {
                StatementKind::Expression(expression) | StatementKind::Return(expression) => {
                    self.writes_register(expression, register, inlined_depth + 1)
                }
            })
    }

    /// Returns the register holding `expression`'s value.
    pub(crate) fn lower_expression(&mut self, expression: &Expression) -> u16 {
        self.lower_expression_to(expression, None)
    }

    /// Lowers a send's argument into `slot`, its argument slot, and answers
    /// the argument's source: `slot` itself when the value was produced
    /// there. A send that is not inlined and an inlined integer leaf write
    /// their `destination` into the slot, and a literal, `self`, a read of
    /// another frame's local or of a vector, a closure and an array are
    /// loaded into it, so the send copies none of them. A named local
    /// answers its own register, which the send copies -- or, when an
    /// argument after it can write that local, the protective `Move` into
    /// the slot, which the send then does not copy. Every other expression
    /// answers the register its value is in -- the result register of an
    /// inlined conditional, `and:`/`or:`, loop or mirror send, an
    /// assignment's value, a cascade's last send -- and the send copies it
    /// into the slot.
    ///
    /// Writing the slot this early is sound because the slots were
    /// allocated before any argument was lowered (`lower_arguments`):
    /// whatever the arguments after this one emit lands above all of them,
    /// and nothing but the send reads a slot.
    fn lower_expression_into(
        &mut self,
        expression: &Expression,
        rest: &[Expression],
        slot: u16,
    ) -> u16 {
        let register = self.lower_expression_to(expression, Some(slot));
        let clobbered = self.clobbered_by(register, rest);
        self.materialize_into(register, clobbered, slot)
    }

    /// The register an instruction that produces a value writes: `into`,
    /// the argument slot `lower_expression_into` lowers into, or else a
    /// fresh scratch register.
    fn target_register(&mut self, into: Option<u16>) -> u16 {
        match into {
            Some(slot) => slot,
            None => self.allocate_scratch(),
        }
    }

    /// `lower_expression`, the value produced in `into` where its last
    /// instruction is one that writes a register of its choosing
    /// (`lower_expression_into`); answers the register holding the value.
    fn lower_expression_to(&mut self, expression: &Expression, into: Option<u16>) -> u16 {
        match &expression.kind {
            ExpressionKind::Literal(literal) => {
                let destination = self.target_register(into);
                self.emit(Instruction::LoadLiteral {
                    destination,
                    literal: lower_literal(literal),
                });
                destination
            }
            ExpressionKind::Identifier(id) => self.lower_identifier_read(id, into),
            ExpressionKind::Assign { target, value, .. } => {
                self.lower_assign(target, value, ValueUse::Read)
            }
            ExpressionKind::Send(send) => self.lower_send(send, ValueUse::Read, into),
            ExpressionKind::Cascade(cascade) => self.lower_cascade(cascade),
            ExpressionKind::Block(block) => self.lower_block_literal(block, into),
            ExpressionKind::DynamicArray { elements, .. } => {
                let element_registers: Vec<u16> = (0..elements.len())
                    .map(|index| self.lower_operand(&elements[index], &elements[index + 1..]))
                    .collect();
                let destination = self.target_register(into);
                self.emit(Instruction::MakeArray {
                    destination,
                    element_registers,
                });
                destination
            }
        }
    }

    /// The position in `scopes` of the innermost converted `Frame` -- the
    /// frame whose layout holds every converted variable this body can see.
    /// From inside a fallback closure that is the frame below it, the one
    /// whose common path the fallback stands in for; the root body is always
    /// converted, so there is always one.
    fn innermost_converted(&self) -> usize {
        self.scopes
            .iter()
            .rposition(|scope| {
                matches!(
                    scope,
                    ScopeKind::Frame {
                        by_reference: false,
                        ..
                    }
                )
            })
            .expect("internal: no converted frame below this body (the root body is converted)")
    }

    /// Maps a resolver `Binding::Local`/`OuterLocal` onto the runtime
    /// register file. The one place a binding becomes an access: a variable a
    /// by-reference frame declares is reached by plain hops, and one a
    /// converted frame declares through that frame's layout -- its own
    /// register, the register it was copied into, or its vector register and
    /// a slot -- with hops added when the reading body is a fallback closure
    /// below it.
    fn resolve_variable(&self, depth: u16, index: u16) -> Access {
        let (declaring_position, key, by_reference) = declaring_scope(&self.scopes, depth);
        if by_reference {
            return access_through(
                hops_above(&self.scopes, declaring_position),
                Location::Register(index),
            );
        }
        let anchor_position = self.innermost_converted();
        let hops = hops_above(&self.scopes, anchor_position);
        let layout = if hops == 0 {
            &self.layout
        } else {
            // `layouts` is innermost last and holds no entry for this body's
            // own frame, so the frame `hops` scopes out sits `hops` entries
            // before the end.
            &self.layouts[self.layouts.len() - hops as usize]
        };
        access_through(hops, layout.locate(key, index, &self.captures))
    }

    /// Emits whatever `access` needs to leave the variable's value in a
    /// register, and answers that register.
    fn emit_read(&mut self, access: Access, into: Option<u16>) -> u16 {
        match access {
            Access::Register(register) => register,
            Access::OuterRegister { hops, index } => {
                let destination = self.target_register(into);
                self.emit(Instruction::LoadOuterLocal {
                    destination,
                    depth: hops,
                    index,
                });
                destination
            }
            Access::Vector { vector, slot } => {
                let vector = self.emit_vector_register(vector);
                let destination = self.target_register(into);
                self.emit(Instruction::LoadVector {
                    destination,
                    vector,
                    index: slot,
                });
                destination
            }
        }
    }

    /// Emits whatever `access` needs to write `source` into the variable.
    /// The caller checks that a register write is allowed; a vector slot is
    /// writable by construction, since only a variable something writes ever
    /// goes into a vector.
    fn emit_store(&mut self, access: Access, source: u16) {
        match access {
            Access::Register(register) => {
                self.emit(Instruction::StoreLocal {
                    source,
                    slot: register,
                });
            }
            Access::OuterRegister { hops, index } => {
                self.emit(Instruction::StoreOuterLocal {
                    source,
                    depth: hops,
                    index,
                });
            }
            Access::Vector { vector, slot } => {
                let vector = self.emit_vector_register(vector);
                self.emit(Instruction::StoreVector {
                    vector,
                    index: slot,
                    source,
                });
            }
        }
    }

    /// The register holding the indirection vector itself: this frame's own
    /// vector register, or -- inside a fallback closure -- a scratch register
    /// the hop to the frame that owns it lands in.
    fn emit_vector_register(&mut self, vector: VectorAccess) -> u16 {
        match vector {
            VectorAccess::Register(register) => register,
            VectorAccess::Outer { hops, index } => {
                let destination = self.allocate_scratch();
                self.emit(Instruction::LoadOuterLocal {
                    destination,
                    depth: hops,
                    index,
                });
                destination
            }
        }
    }

    fn lower_identifier_read(&mut self, id: &ast::Identifier, into: Option<u16>) -> u16 {
        if id.name == "self" {
            let destination = self.target_register(into);
            self.emit(Instruction::LoadSelf { destination });
            return destination;
        }
        match self.bindings.get(&id.span) {
            Some(Binding::Local(slot)) => {
                let access = self.resolve_variable(0, *slot);
                self.emit_read(access, into)
            }
            Some(Binding::OuterLocal { depth, index }) => {
                let access = self.resolve_variable(*depth, *index);
                self.emit_read(access, into)
            }
            Some(Binding::LexicalSend { depth }) => {
                let depth = *depth;
                let (argument_base, argument_sources) = self.lower_arguments(&[], &[]);
                self.emit_lexical_send(
                    depth,
                    crate::Symbol::intern(&id.name),
                    argument_base,
                    argument_sources,
                    into,
                )
            }
            // The resolver attaches `SuperSend` to a bare identifier in one
            // place: a `super`-prefixed inheritance-clause receiver, `super
            // Foo`. §6.1 evaluates it "as if it occurred inside an instance
            // method" of the enclosing class, so it is the super send of
            // `Foo`, whose lookup starts above the clause activation's
            // `current_class` -- the class that declares the clause.
            Some(Binding::SuperSend) => {
                let (argument_base, argument_sources) = self.lower_arguments(&[], &[]);
                self.emit_send(into, |destination, call_site| Instruction::SendSuper {
                    selector: crate::Symbol::intern(&id.name),
                    argument_base,
                    argument_sources,
                    destination,
                    call_site,
                })
            }
            // `MirrorSend` and `EnclosingObject` mark a send, never an
            // identifier.
            //
            // Not a compiler-invariant violation: `resolve_identifier`
            // deliberately records no binding for `super`, and the lexer
            // deliberately does not reserve it, so `^super` is a program the
            // language accepts and rejects at *evaluation* time. A module
            // root's clause receivers, which the resolver leaves unbound for
            // the root rule, land here too and are never run. Compilation
            // cannot raise a `NewspeakError`, so the error is deferred as
            // `UnresolvedIdentifier`: the VM raises `literals`' own error when
            // the instruction executes.
            Some(Binding::MirrorSend | Binding::EnclosingObject { .. }) | None => {
                let destination = self.target_register(into);
                self.emit(Instruction::LoadLiteral {
                    destination,
                    literal: BytecodeLiteral::UnresolvedIdentifier(id.name.clone()),
                });
                destination
            }
        }
    }

    /// Every write into a named local, checked against the table of the
    /// frame that owns the register: this frame's own for a `StoreLocal`,
    /// and the target frame's for a `StoreOuterLocal` -- the
    /// write-through-a-closure case `materialize`'s copy exists to protect
    /// against (`CLAUDE.md`, "An expression's value register must still hold
    /// that value"). A vector slot needs no check: only a variable something
    /// writes is ever put in a vector.
    fn check_writable(&self, access: Access) {
        let read_only = match access {
            Access::Register(register) => self.read_only_locals[register as usize],
            // `enclosing_read_only_locals` is innermost last, so the frame
            // `hops` scopes out sits `hops` entries before the end.
            Access::OuterRegister { hops, index } => {
                self.enclosing_read_only_locals
                    [self.enclosing_read_only_locals.len() - hops as usize][index as usize]
            }
            Access::Vector { .. } => false,
        };
        debug_assert!(
            !read_only,
            "internal: an assignment bound to a parameter or an immutable local \
             (the resolver passes a setter send by them)"
        );
    }

    /// `use_` is `Discarded` only where nothing reads this assignment's
    /// value -- see `lower_expression_for`, the only caller that passes it.
    fn lower_assign(
        &mut self,
        target: &ast::Identifier,
        value: &Expression,
        use_: ValueUse,
    ) -> u16 {
        let mut value_register = self.lower_expression(value);
        match self.bindings.get(&target.span) {
            Some(Binding::Local(slot)) => {
                let access = self.resolve_variable(0, *slot);
                self.check_writable(access);
                self.emit_store(access, value_register);
            }
            Some(Binding::OuterLocal { depth, index }) => {
                let access = self.resolve_variable(*depth, *index);
                self.check_writable(access);
                self.emit_store(access, value_register);
            }
            Some(Binding::LexicalSend { depth }) => {
                let depth = *depth;
                let setter = crate::Symbol::intern(&format!("{}:", target.name));
                // The setter's one argument slot, after the value.
                let argument_base = self.allocate_argument_slots(&[value_register], 1);
                // A setter send runs user code before anything reads this
                // expression's own value, so a named local's register may no
                // longer hold what was assigned by then -- but only where that
                // code can reach the register at all, which after conversion
                // is a fallback closure's locals alone (`a_send_can_write`).
                // The copy goes into the slot, which is then the setter's
                // argument and the assignment's value. (The
                // `StoreLocal`/`StoreOuterLocal` cases above need no copy: the
                // only named local a `StoreLocal` can write while
                // `value_register` is live is `value_register` itself -- `t::
                // t` -- which leaves it unchanged, and a `StoreOuterLocal`
                // writes a different frame.)
                let clobbered = use_ == ValueUse::Read && self.a_send_can_write(value_register);
                value_register = self.materialize_into(value_register, clobbered, argument_base);
                self.emit_lexical_send(depth, setter, argument_base, vec![value_register], None);
            }
            Some(Binding::SuperSend | Binding::MirrorSend | Binding::EnclosingObject { .. })
            | None => panic!(
                "internal: unresolved setter target `{}` (the resolver should have rejected this)",
                target.name
            ),
        }
        // An assignment's own value is `value`'s value
        // (`docs/design/dispatch.md`, "Setter sends") -- already sitting in
        // register `value_register`.
        value_register
    }

    /// The next send's number within this family. Numbers are handed out in
    /// emission order across the root and every block nested in it, so a
    /// family's sends are dense from 0 and none shares a cache entry
    /// (`docs/design/dispatch.md`, "Code families").
    fn take_call_site(&mut self) -> CallSite {
        let number = self.next_call_site;
        self.next_call_site = number
            .checked_add(1)
            .filter(|next| *next < CallSite::UNCACHED.number)
            .expect("internal: more than u32::MAX - 1 sends in one code family");
        CallSite { number }
    }

    /// Emits a send whose value lands in `into`, or else in a fresh
    /// scratch register.
    fn emit_send(
        &mut self,
        into: Option<u16>,
        build: impl FnOnce(u16, CallSite) -> Instruction,
    ) -> u16 {
        let destination = self.target_register(into);
        let call_site = self.take_call_site();
        self.emit(build(destination, call_site));
        destination
    }

    fn emit_lexical_send(
        &mut self,
        depth: u16,
        selector: crate::Symbol,
        argument_base: u16,
        argument_sources: Vec<u16>,
        into: Option<u16>,
    ) -> u16 {
        self.emit_send(into, |destination, call_site| Instruction::SendLexical {
            depth,
            selector,
            argument_base,
            argument_sources,
            destination,
            call_site,
        })
    }

    pub(crate) fn emit_ordinary_send_with_receiver(
        &mut self,
        receiver_register: u16,
        message: &ast::MessageSend,
        into: Option<u16>,
    ) -> u16 {
        let selector = selector_of(message);
        let (argument_base, argument_sources) =
            self.lower_arguments(&[receiver_register], &message.arguments);
        self.emit_send(into, |destination, call_site| Instruction::SendPublic {
            selector,
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            call_site,
        })
    }

    /// The real send an inlined conditional falls back to when its receiver
    /// is not a Boolean. The closures it builds out of the same block
    /// literals keep the by-reference model: such a closure can be kept and
    /// run long after this statement (`docs/design/execution.md`, "Fallback
    /// blocks").
    fn lower_fallback_send(&mut self, receiver_register: u16, message: &ast::MessageSend) -> u16 {
        let saved = std::mem::replace(&mut self.blocks_by_reference, true);
        let result = self.emit_ordinary_send_with_receiver(receiver_register, message, None);
        self.blocks_by_reference = saved;
        result
    }

    /// A send's `count` contiguous argument slots, allocated at once, after
    /// every scratch register in `receivers` the send also reads -- a
    /// receiver, a mirror send's subject, a setter's value -- and before
    /// any of its arguments is lowered. Answers the first slot; for no
    /// argument, where the first would be.
    ///
    /// The slots are not always the top of what is live at the send:
    /// whatever an argument is evaluated with lands above them, and an
    /// argument the send copies (`lower_expression_into`) -- the result
    /// register of an inlined conditional, `and:`/`or:`, loop or mirror
    /// send, an assignment's value or a cascade's last send, unless that is a named local below the slots -- is read by the
    /// send from a register above them. Every other register above the
    /// slots is dead by the time the send executes
    /// (`docs/design/compiler.md`, "Sends").
    fn allocate_argument_slots(&mut self, receivers: &[u16], count: usize) -> u16 {
        let argument_base = self.next_scratch;
        for _ in 0..count {
            self.allocate_scratch();
        }
        debug_assert!(
            argument_base >= self.base,
            "internal: argument slots from r{argument_base} below the first scratch register r{}",
            self.base
        );
        debug_assert!(
            self.too_large
                || receivers
                    .iter()
                    .all(|receiver| *receiver < self.base || *receiver < argument_base),
            "internal: argument slots from r{argument_base} not above the send's scratch operands {receivers:?}"
        );
        argument_base
    }

    /// A send's arguments: their slots allocated first
    /// (`allocate_argument_slots`), then each argument lowered into its
    /// slot (`lower_expression_into`), the register it answers recorded as
    /// its source -- every argument is live until the send reads it, so
    /// each has to survive whatever the arguments after it emit, which is
    /// why a named local a later argument can write is copied, into its
    /// slot, as `lower_operand` copies an operand. The send copies each
    /// source that is not its slot. Answers the first slot and the sources.
    fn lower_arguments(&mut self, receivers: &[u16], arguments: &[Expression]) -> (u16, Vec<u16>) {
        let argument_base = self.allocate_argument_slots(receivers, arguments.len());
        let argument_sources = (0..arguments.len())
            .map(|index| {
                // Past `u16::MAX` the body is too large and nothing emitted
                // is kept (`finish`), so a saturated slot is never run.
                let offset = u16::try_from(index).unwrap_or(u16::MAX);
                let slot = argument_base.saturating_add(offset);
                self.lower_expression_into(&arguments[index], &arguments[index + 1..], slot)
            })
            .collect();
        (argument_base, argument_sources)
    }

    /// `use_` is what reads the send's value. Only an inlined form passes it
    /// on, to the blocks whose values become its own.
    fn lower_send(&mut self, send: &ast::Send, use_: ValueUse, into: Option<u16>) -> u16 {
        // The inlining gate: only an ordinary explicit-receiver send is ever
        // offered to `recognize` (`docs/design/compiler.md`, "Control-flow
        // inlining"), through the one predicate the capture analysis and the
        // protective-copy walk ask too. Every recognized form still keeps a
        // runtime guard whose failure path is a real virtual send, so
        // inlining stays a pure optimization.
        if !control_flow::is_ordinary_send(self.bindings, send) {
            return self.lower_super_or_lexical_send(send, into);
        }
        if let Some((primitive, block)) = control_flow::recognize_mirror_send(self.bindings, send) {
            return self.lower_mirror_send(send, primitive, block, use_);
        }
        if let Some(form) = control_flow::recognize(send) {
            return match form {
                InlineForm::IfTrueIfFalse(true_block, false_block) => {
                    self.lower_if(send, Some(true_block), Some(false_block), use_)
                }
                InlineForm::IfTrue(true_block) => self.lower_if(send, Some(true_block), None, use_),
                InlineForm::IfFalse(false_block) => {
                    self.lower_if(send, None, Some(false_block), use_)
                }
                InlineForm::And(block) => self.lower_and_or(send, block, true, use_),
                InlineForm::Or(block) => self.lower_and_or(send, block, false, use_),
                InlineForm::WhileTrue { condition, body } => {
                    self.lower_while(condition, body, true)
                }
                InlineForm::WhileFalse { condition, body } => {
                    self.lower_while(condition, body, false)
                }
            };
        }
        if let Some(operation) = control_flow::recognize_integer_binary(send) {
            return self.lower_integer_binary(send, operation, into);
        }
        // The receiver is read by the send instruction, so it has to
        // survive its own arguments' lowering.
        let receiver_register = self.lower_operand(&send.receiver, &send.message.arguments);
        self.emit_ordinary_send_with_receiver(receiver_register, &send.message, into)
    }

    /// The sends `is_ordinary_send` rejects: none has a receiver expression,
    /// and none is ever inlined. `outer N` alone is no send at all but the
    /// object `outer N m` would send to, loaded without running any code.
    fn lower_super_or_lexical_send(&mut self, send: &ast::Send, into: Option<u16>) -> u16 {
        let selector = selector_of(&send.message);
        match self.bindings.get(&send.span) {
            Some(Binding::EnclosingObject { depth }) => {
                let destination = self.target_register(into);
                self.emit(Instruction::LoadEnclosingObject {
                    destination,
                    depth: *depth,
                });
                destination
            }
            Some(Binding::LexicalSend { depth }) => {
                let depth = *depth;
                let (argument_base, argument_sources) =
                    self.lower_arguments(&[], &send.message.arguments);
                self.emit_lexical_send(depth, selector, argument_base, argument_sources, into)
            }
            binding => {
                debug_assert!(
                    matches!(binding, Some(Binding::SuperSend)),
                    "internal: is_ordinary_send rejected a send that is neither super nor lexical"
                );
                let (argument_base, argument_sources) =
                    self.lower_arguments(&[], &send.message.arguments);
                self.emit_send(into, |destination, call_site| Instruction::SendSuper {
                    selector,
                    argument_base,
                    argument_sources,
                    destination,
                    call_site,
                })
            }
        }
    }

    fn lower_cascade(&mut self, cascade: &ast::Cascade) -> u16 {
        let receiver_register = self.lower_expression(&cascade.receiver);
        // The one receiver register is read again by every message of the
        // cascade, so it has to survive the earlier *sends* too -- the last
        // one reads it as it executes -- and the arguments of every message,
        // not only the first's.
        let clobbered = (!cascade.rest.is_empty() && self.a_send_can_write(receiver_register))
            || std::iter::once(&cascade.first)
                .chain(cascade.rest.iter())
                .flat_map(|message| message.arguments.iter())
                .any(|argument| self.writes_register(argument, receiver_register, 0));
        let receiver_register = self.materialize(receiver_register, clobbered);
        let mut result =
            self.emit_ordinary_send_with_receiver(receiver_register, &cascade.first, None);
        for message in &cascade.rest {
            result = self.emit_ordinary_send_with_receiver(receiver_register, message, None);
        }
        result
    }

    /// The index the next emitted instruction will get -- a forward label.
    /// Passing `u32::MAX` would take a body of gigabytes of source, near
    /// the four gibibytes a `Span`'s `u32` offsets address; were one to, a
    /// wrapped label would be a jump to the wrong instruction, so it panics
    /// instead.
    fn here(&self) -> u32 {
        u32::try_from(self.code.len())
            .expect("internal: more instructions in one body than a u32 numbers")
    }

    fn patch_target(&mut self, instruction_index: usize, target: u32) {
        self.code.patch_target(instruction_index, target);
    }

    /// Lowers `block`'s statements directly into the current instruction stream
    /// -- no `MakeClosure`, no new activation. The block's statements are
    /// sub-expressions of the enclosing statement: no
    /// `statement_program_counter` entries, and the scratch allocator resets
    /// only to the watermark at the block's start (never to `base`, which would
    /// clobber the enclosing statement's live registers). Pushes
    /// `ScopeKind::Inlined` so local references inside (and in nested real
    /// blocks) drop one frame hop. Returns the register holding the block's
    /// value (`nil` if it has no statements); `use_` is what reads it, and so
    /// what reads the last statement's value.
    fn lower_inline_block_body(&mut self, block: &ast::BlockExpression, use_: ValueUse) -> u16 {
        debug_assert!(
            block.parameters.is_empty() && block.body.temporaries.is_empty(),
            "internal: the inlining gate admits only parameterless, temp-less blocks"
        );
        self.scopes.push(ScopeKind::Inlined);
        let mark = self.next_scratch;
        let mut result = None;
        for (index, statement) in block.body.statements.iter().enumerate() {
            self.next_scratch = mark;
            let statement_use = if index + 1 == block.body.statements.len() {
                use_
            } else {
                ValueUse::Discarded
            };
            result = self.lower_statement_inner(statement, statement_use);
        }
        self.scopes.pop();
        match result {
            Some(register) => register,
            None => self.emit_nil(),
        }
    }

    /// The guard-failure path of an inlined loop: signals
    /// `NonBooleanReceiver`, as the native `whileTrue:`/`whileFalse:` does
    /// with a non-Boolean condition value. The path ends here.
    fn emit_signal_non_boolean(&mut self, register: u16) {
        self.emit(Instruction::SignalNonBoolean { register });
    }

    /// The shared shape behind every one-shot inlined form: `guard_register`
    /// exactly `Boolean(true)` runs `on_true`, exactly `Boolean(false)` runs
    /// `on_false`, anything else runs `on_fallback` (a real send). Each
    /// returns the register holding its value; the chosen one is `Move`d into
    /// `destination`.
    ///
    /// The three arms are mutually exclusive -- at most one of them ever
    /// executes for a given evaluation -- so the scratch allocator is reset to
    /// its pre-arm mark before each one, letting arms reuse the same registers
    /// instead of stacking their high-water marks. `guard_register` and
    /// `destination` (and any already-evaluated sibling register, e.g. another
    /// argument of the enclosing send) are all allocated before `mark` is
    /// taken, so the reset can never touch a register that outlives its arm.
    fn emit_guarded_branch(
        &mut self,
        guard_register: u16,
        destination: u16,
        on_true: impl FnOnce(&mut Self) -> u16,
        on_false: impl FnOnce(&mut Self) -> u16,
        on_fallback: impl FnOnce(&mut Self) -> u16,
    ) {
        let mark = self.next_scratch;
        let not_boolean_jump = self.emit(Instruction::JumpIfNotBoolean {
            register: guard_register,
            target: 0,
        });
        let false_jump = self.emit(Instruction::BranchIfFalse {
            register: guard_register,
            target: 0,
        });
        self.next_scratch = mark;
        let true_result = on_true(self);
        self.emit(Instruction::Move {
            destination,
            source: true_result,
        });
        let after_true_jump = self.emit(Instruction::Jump { target: 0 });
        let false_label = self.here();
        self.patch_target(false_jump, false_label);
        self.next_scratch = mark;
        let false_result = on_false(self);
        self.emit(Instruction::Move {
            destination,
            source: false_result,
        });
        let after_false_jump = self.emit(Instruction::Jump { target: 0 });
        let fallback_label = self.here();
        self.patch_target(not_boolean_jump, fallback_label);
        self.next_scratch = mark;
        let fallback_result = on_fallback(self);
        self.emit(Instruction::Move {
            destination,
            source: fallback_result,
        });
        let end_label = self.here();
        self.patch_target(after_true_jump, end_label);
        self.patch_target(after_false_jump, end_label);
        self.next_scratch = mark;
    }

    /// `ifTrue:`/`ifFalse:`/`ifTrue:ifFalse:`/`ifFalse:ifTrue:`: an absent
    /// branch is `nil`, matching `True>>ifFalse:` and `False>>ifTrue:`,
    /// which answer `nil`. A branch's value is only moved into the send's
    /// own result, so `use_` -- what reads that result -- is what reads the
    /// branch's value.
    fn lower_if(
        &mut self,
        send: &ast::Send,
        true_block: Option<&ast::BlockExpression>,
        false_block: Option<&ast::BlockExpression>,
        use_: ValueUse,
    ) -> u16 {
        let guard_register = self.lower_expression(&send.receiver);
        let destination = self.allocate_scratch();
        self.emit_guarded_branch(
            guard_register,
            destination,
            |lowerer| match true_block {
                Some(block) => lowerer.lower_inline_block_body(block, use_),
                None => lowerer.emit_nil(),
            },
            |lowerer| match false_block {
                Some(block) => lowerer.lower_inline_block_body(block, use_),
                None => lowerer.emit_nil(),
            },
            |lowerer| lowerer.lower_fallback_send(guard_register, &send.message),
        );
        destination
    }

    /// An integer leaf's inlined fast path: two ways where
    /// `emit_guarded_branch` has three, because the instruction is its own
    /// guard and its `target` is the fallback.
    ///
    /// The receiver is lowered with `lower_operand` against the argument,
    /// because whatever the argument lowers to may write the receiver's
    /// register -- the same question `lower_arguments` asks for every send.
    ///
    /// The fallback reads the two registers the guard already filled instead
    /// of lowering the message a second time: re-lowering it would evaluate
    /// the argument's own side effects twice whenever the guard fails. What
    /// it emits is therefore the `SendPublic` the ordinary lowering would
    /// have emitted for this send, over the same receiver and argument
    /// values -- which is what makes the inlining a pure optimization, with every
    /// error and its text still coming from that send's primitive. It lands
    /// its result in the same `destination` the fast path writes, rather
    /// than in a scratch register of its own that a `Move` then copies: the
    /// two paths are mutually exclusive, and `destination` is neither
    /// operand: a fresh register allocated after both, or `into`, the
    /// argument slot the operation is lowered into
    /// (`lower_expression_into`), which was allocated before both.
    fn lower_integer_binary(
        &mut self,
        send: &ast::Send,
        operation: IntegerOperation,
        into: Option<u16>,
    ) -> u16 {
        // The parser gives a binary selector exactly one argument; name the
        // invariant so a mismatch surfaces here rather than as a bare
        // slice-index panic, as `control_flow::recognize` does for its own.
        debug_assert_eq!(
            send.message.arguments.len(),
            1,
            "internal: a binary selector takes exactly one argument, got {}",
            send.message.arguments.len()
        );
        let argument = &send.message.arguments[0];
        let left_register = self.lower_operand(&send.receiver, std::slice::from_ref(argument));
        let right_register = self.lower_expression(argument);
        // The fallback's one argument slot: the argument's own register when
        // it is a scratch register above the receiver, where the slot would
        // stand, and otherwise -- a named local -- a slot of its own, which
        // the send copies the argument into.
        let argument_base = if right_register >= self.base && right_register > left_register {
            right_register
        } else {
            self.allocate_argument_slots(&[left_register, right_register], 1)
        };
        let destination = self.target_register(into);
        let fallback_jump = self.emit(Instruction::IntegerBinary {
            operation,
            destination,
            left: left_register,
            right: right_register,
            target: 0,
        });
        let after_fast_path_jump = self.emit(Instruction::Jump { target: 0 });
        let fallback_label = self.here();
        self.patch_target(fallback_jump, fallback_label);
        let call_site = self.take_call_site();
        self.emit(Instruction::SendPublic {
            selector: selector_of(&send.message),
            receiver_register: left_register,
            argument_base,
            argument_sources: vec![right_register],
            destination,
            call_site,
        });
        let end_label = self.here();
        self.patch_target(after_fast_path_jump, end_label);
        destination
    }

    /// A mirror send with its failure block inline: `SendMirror`, then a jump
    /// over the block, then the block's body lowered into this body -- a `^`
    /// in it returns from the kernel method, as the failure path of a
    /// primitive method's body should (`docs/design/compiler.md`, "Mirror
    /// sends").
    ///
    /// The receiver, the subject and the leaf's arguments are lowered in
    /// order, each protected against what is lowered after it, as
    /// `lower_arguments` protects a send's arguments. The block is not
    /// lowered as a value: it is code at `failure`, run only when the leaf
    /// declines. Its value lands in the same `destination` the leaf writes,
    /// the two paths being mutually exclusive, and it is `Move`d there as
    /// `emit_guarded_branch` moves a branch's value -- after a block that
    /// ends in `^` that `Move` is unreachable, and harmless.
    ///
    /// Unlike an inlined conditional there is no fallback send: the mirror's
    /// class and method table are fixed, so nothing but this block can ever
    /// answer the leaf's failure, and no closure is built out of it.
    fn lower_mirror_send(
        &mut self,
        send: &ast::Send,
        primitive: MirrorPrimitive,
        block: &ast::BlockExpression,
        use_: ValueUse,
    ) -> u16 {
        let arguments = &send.message.arguments;
        let leaf = &arguments[..arguments.len() - 1];
        let mirror_register = self.lower_operand(&send.receiver, leaf);
        let [subject, rest @ ..] = leaf else {
            unreachable!("internal: every mirror selector takes a subject");
        };
        let subject_register = self.lower_operand(subject, rest);
        let (argument_base, argument_sources) =
            self.lower_arguments(&[mirror_register, subject_register], rest);
        let destination = self.allocate_scratch();
        let failure_jump = self.emit(Instruction::SendMirror {
            primitive,
            mirror_register,
            subject_register,
            argument_base,
            argument_sources,
            destination,
            failure: 0,
        });
        let after_send_jump = self.emit(Instruction::Jump { target: 0 });
        let failure_label = self.here();
        self.patch_target(failure_jump, failure_label);
        let failure_result = self.lower_inline_block_body(block, use_);
        self.emit(Instruction::Move {
            destination,
            source: failure_result,
        });
        let end_label = self.here();
        self.patch_target(after_send_jump, end_label);
        destination
    }

    /// `and:`/`or:`: the short-circuit arm is the receiver's own Boolean
    /// value, the other arm the block's value as it is. A branch's value is
    /// only moved into the send's own result, so `use_` -- what reads that
    /// result -- is what reads the block's value, as in `lower_if`.
    fn lower_and_or(
        &mut self,
        send: &ast::Send,
        block: &ast::BlockExpression,
        is_and: bool,
        use_: ValueUse,
    ) -> u16 {
        let guard_register = self.lower_expression(&send.receiver);
        let destination = self.allocate_scratch();
        let evaluate = |lowerer: &mut Self| lowerer.lower_inline_block_body(block, use_);
        let fallback =
            |lowerer: &mut Self| lowerer.lower_fallback_send(guard_register, &send.message);
        if is_and {
            self.emit_guarded_branch(
                guard_register,
                destination,
                evaluate,
                |_| guard_register,
                fallback,
            );
        } else {
            self.emit_guarded_branch(
                guard_register,
                destination,
                |_| guard_register,
                evaluate,
                fallback,
            );
        }
        destination
    }

    /// A real loop: condition, guard, exit branch, body, back-edge. A
    /// non-Boolean condition value takes the path that signals
    /// `NonBooleanReceiver`, which never reaches the loop's end; a normal exit
    /// leaves `nil` in `destination`. The guard reads the condition's value on
    /// every iteration, whatever reads the loop's; nothing reads the body's.
    fn lower_while(
        &mut self,
        condition: &ast::BlockExpression,
        body: &ast::BlockExpression,
        while_true: bool,
    ) -> u16 {
        let destination = self.emit_nil();
        let loop_start = self.here();
        let guard_register = self.lower_inline_block_body(condition, ValueUse::Read);
        let not_boolean_jump = self.emit(Instruction::JumpIfNotBoolean {
            register: guard_register,
            target: 0,
        });
        let exit_jump = if while_true {
            self.emit(Instruction::BranchIfFalse {
                register: guard_register,
                target: 0,
            })
        } else {
            self.emit(Instruction::BranchIfTrue {
                register: guard_register,
                target: 0,
            })
        };
        self.lower_inline_block_body(body, ValueUse::Discarded);
        self.emit(Instruction::Jump { target: loop_start });
        let fallback_label = self.here();
        self.patch_target(not_boolean_jump, fallback_label);
        // The signal ends its path, like a return: nothing writes
        // `destination` after it, and the end label is reached only by the
        // exit branch.
        self.emit_signal_non_boolean(guard_register);
        let end_label = self.here();
        self.patch_target(exit_jump, end_label);
        destination
    }

    pub(crate) fn finish(self, source: Span) -> CompiledCode {
        if self.too_large {
            return super::body_too_large(self.kind, self.argument_count, source);
        }
        // Control-flow inlining and the integer leaves' fast path are the two
        // sources of a patched target, and every one of their labels is a
        // `here()` that `finish_body`'s terminator guarantees is a real
        // instruction index, so a target must be in range. Every instruction
        // `patch_target` accepts is listed below, and the two lists are one
        // defence: a third emitter that patches a target has to appear in
        // both. This check does *not* catch a never-patched target: an
        // unpatched `target` field stays `0`, which is always in range and
        // would pass silently here. Completeness against that failure mode
        // instead comes from each emitter's own emit/patch pairing (every
        // `emit(... { target: 0 })` in this module is paired with a
        // `patch_target` call before `finish` runs) -- an out-of-range target
        // is a compiler bug, not a program error, so it is a `debug_assert!`
        // -- checked on every debug test run, free in release.
        debug_assert!(
            self.code
                .instructions()
                .iter()
                .filter_map(|instruction| match instruction {
                    Instruction::Jump { target }
                    | Instruction::JumpIfNotBoolean { target, .. }
                    | Instruction::BranchIfFalse { target, .. }
                    | Instruction::BranchIfTrue { target, .. }
                    | Instruction::IntegerBinary { target, .. }
                    | Instruction::SendMirror {
                        failure: target, ..
                    } => Some(*target as usize),
                    _ => None,
                })
                .all(|target| target < self.code.len()),
            "internal: a jump target does not name an instruction of this body"
        );
        CompiledCode::from_builder(
            self.kind,
            self.code,
            self.argument_count,
            self.max_register,
            self.layout.copied_count,
            self.statement_program_counter,
            source,
        )
    }
}

pub(crate) fn selector_of(message: &ast::MessageSend) -> crate::Symbol {
    match &message.selector {
        ast::Selector::Unary(selector_name)
        | ast::Selector::Binary(selector_name)
        | ast::Selector::Keyword(selector_name) => crate::Symbol::intern(selector_name),
    }
}

pub(super) fn lower_literal(literal: &ast::Literal) -> BytecodeLiteral {
    match literal {
        // Parsed once, here. The parser admits only integer literals that
        // denote a value (§5.1.1 bounds none; `check_radix_number` refuses a
        // radix or digit that denotes nothing), so a refusal is a compiler
        // invariant, not a program error.
        ast::Literal::Integer { text, .. } => match parse_integer_literal(text) {
            Ok(IntegerLiteral::Small(integer)) => BytecodeLiteral::SmallInteger(integer),
            Ok(IntegerLiteral::Large(integer)) => BytecodeLiteral::LargeInteger(integer),
            Err(message) => {
                panic!("the parser admitted an integer literal that denotes nothing: {message}")
            }
        },
        ast::Literal::Float { text, .. } => BytecodeLiteral::Float(parse_float_literal(text)),
        ast::Literal::String { value, .. } => BytecodeLiteral::String(value.clone()),
        ast::Literal::Symbol { value, .. } => BytecodeLiteral::Symbol(crate::Symbol::intern(value)),
        ast::Literal::Boolean { value, .. } => BytecodeLiteral::Boolean(*value),
        ast::Literal::Nil { .. } => BytecodeLiteral::Nil,
        ast::Literal::Pattern { .. } => BytecodeLiteral::Nil, // patterns are not implemented (no typechecker)
    }
}

#[cfg(test)]
mod tests {
    use super::super::capture::{analyze, RootBody};
    use super::*;
    use indoc::{formatdoc, indoc};
    use std::path::PathBuf;

    /// A `Lowerer` for a body a test describes directly, over the capture
    /// analysis of that same body.
    fn lowerer_for<'a>(
        kind: CodeKind,
        argument_count: usize,
        temporaries: &[ast::SlotDeclaration],
        slot_initializers: Vec<&Expression>,
        statements: &[Statement],
        bindings: &'a Rc<HashMap<Span, Binding>>,
    ) -> Lowerer<'a> {
        let captures = Rc::new(analyze(
            RootBody {
                argument_count,
                temporaries,
                slot_initializers,
                statements,
            },
            bindings,
        ));
        Lowerer::new(
            kind,
            BodyKey::Root,
            argument_count,
            temporaries,
            bindings,
            captures,
            false,
            Enclosing::default(),
            0,
        )
    }

    /// Parses `source` (a full, minimal `class ... ( ) ( <methods> ) : ()`
    /// module), resolves it for real via `crate::resolve`, and returns the
    /// module's instance-side method `index` plus the real bindings map it
    /// resolved under -- the same "parse a throwaway module" pattern
    /// `dispatch.rs`'s own tests use, so these tests exercise real
    /// `Binding`s rather than hand-built ones.
    pub(super) fn resolve_method(
        source: &str,
        index: usize,
    ) -> (ast::MethodDeclaration, Rc<HashMap<Span, Binding>>) {
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(source),
            crate::SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let method = resolved
            .module
            .root_class
            .instance_side
            .methods()
            .nth(index)
            .expect("a method")
            .clone();
        (method, resolved.bindings.clone())
    }

    pub(super) fn method_lowerer<'a>(
        method: &ast::MethodDeclaration,
        bindings: &'a Rc<HashMap<Span, Binding>>,
    ) -> Lowerer<'a> {
        let argument_count = match &method.pattern {
            ast::MessagePattern::Unary { .. } => 0,
            ast::MessagePattern::Binary { .. } => 1,
            ast::MessagePattern::Keyword { parts } => parts.len(),
        };
        lowerer_for(
            CodeKind::Method,
            argument_count,
            &method.body.temporaries,
            Vec::new(),
            &method.body.statements,
            bindings,
        )
    }

    fn return_expression(method: &ast::MethodDeclaration) -> &Expression {
        let StatementKind::Return(expression) = &method.body.statements[0].kind else {
            panic!("expected a return statement");
        };
        expression
    }

    #[test]
    fn a_literal_returns_a_fresh_scratch_register() {
        let (method, bindings) = resolve_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo = ( ^42 )
                ) : ()
            "},
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings); // 0 parameters, 0 temporaries -> base 0
        let register = lowerer.lower_expression(return_expression(&method));
        assert_eq!(register, 0);
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instructions(),
            vec![Instruction::LoadLiteral {
                destination: 0,
                literal: BytecodeLiteral::SmallInteger(42)
            }]
        );
        assert_eq!(code.register_count, 1);
        assert_eq!(code.kind, CodeKind::Method);
    }

    #[test]
    fn reading_a_parameter_returns_its_own_register_with_no_extra_instruction() {
        let (method, bindings) = resolve_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo: x = ( ^x )
                ) : ()
            "},
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings); // 1 parameter (register 0)
        let register = lowerer.lower_expression(return_expression(&method));
        assert_eq!(
            register, 0,
            "reading a param should return its own register"
        );
        let code = lowerer.finish(method.span);
        assert!(
            code.instructions().is_empty(),
            "no instruction needed to read a param"
        );
        assert_eq!(code.argument_count, 1);
    }

    /// The value-register invariant: the register `lower_expression` returns
    /// must still hold that value when its consumer executes. A named local's
    /// own register is live storage, so an operand that has to survive an
    /// argument which can write it is copied into a scratch first.
    #[test]
    fn an_operand_that_a_later_argument_can_write_is_copied_out_of_its_local() {
        let (method, bindings) = resolve_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo: x bar: y = ( ^x )
                  m = ( | t | t:: 1. ^self foo: t bar: (t:: 9) )
                ) : ()
            "},
            1,
        );
        let mut lowerer = method_lowerer(&method, &bindings); // temporary `t` is register 0
        for statement in &method.body.statements {
            lowerer.lower_statement(statement, ValueUse::Discarded);
        }
        let code = lowerer.finish(method.span);
        // An explicit `self` receiver is a depth-0 lexical send.
        let instructions = code.instructions();
        let Some(Instruction::SendLexical {
            argument_sources, ..
        }) = instructions
            .iter()
            .find(|instruction| matches!(instruction, Instruction::SendLexical { .. }))
        else {
            panic!("expected the keyword send");
        };
        assert_ne!(
            argument_sources[0], 0,
            "`t` is register 0; the first argument must not be its live register"
        );
        assert!(
            code.instructions().contains(&Instruction::Move {
                destination: argument_sources[0],
                source: 0
            }),
            "the copy out of `t` is a Move emitted before the assignment"
        );
    }

    /// ... and the fast path is kept: an operand no later operand can write
    /// still costs zero instructions.
    #[test]
    fn an_operand_no_later_argument_can_write_stays_in_its_own_register() {
        let (method, bindings) = resolve_method(
            indoc! {"
                class X = Object new (
                ) (
                  m: a with: b = ( ^a foo: b bar: 3 )
                ) : ()
            "},
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings); // parameters a, b = registers 0, 1
        lowerer.lower_expression(return_expression(&method));
        let code = lowerer.finish(method.span);
        assert!(
            !code
                .instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Move { .. })),
            "no operand needs a copy here: {:?}",
            code.instructions()
        );
        assert_eq!(
            code.instructions()
                .iter()
                .filter_map(|instruction| match instruction {
                    Instruction::SendPublic {
                        receiver_register,
                        argument_sources,
                        ..
                    } => Some((*receiver_register, argument_sources.clone())),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![(0, vec![1, 3])],
            "receiver and first argument are read straight out of their own registers, the literal out of its slot"
        );
    }

    /// A parameter cannot be written once its activation starts (§3.6), so
    /// the send in `rest` that would force a copy out of a mutable local
    /// forces none here.
    #[test]
    fn a_parameter_operand_is_never_copied() {
        let (method, bindings) = resolve_method(
            indoc! {"
                class X = Object new (
                ) (
                  m: argument = ( ^argument foo: self bar )
                ) : ()
            "},
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings); // `argument` is register 0
        lowerer.lower_expression(return_expression(&method));
        let code = lowerer.finish(method.span);
        assert!(
            !code
                .instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Move { .. })),
            "a parameter needs no copy: {:?}",
            code.instructions()
        );
    }

    /// Likewise an immutable local: only its initializer writes it.
    #[test]
    fn an_immutable_local_operand_is_never_copied() {
        let source = indoc! {"
            class X = Object new (
            ) (
              m = ( | local = 1. | ^local foo: self bar )
            ) : ()
        "};
        let code = lower_whole_method(source, 0); // `local` is register 0
        assert!(
            !code
                .instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Move { source: 0, .. })),
            "an immutable local needs no copy: {:?}",
            code.instructions()
        );
    }

    /// The receiver register of the one `SendPublic` whose selector is
    /// `selector`.
    fn public_receiver(code: &CompiledCode, selector: &str) -> u16 {
        let instructions = code.instructions();
        let Some(Instruction::SendPublic {
            receiver_register, ..
        }) = instructions.iter().find(
            |instruction| matches!(instruction, Instruction::SendPublic { selector: sent, .. } if sent.as_str() == selector),
        ) else {
            panic!("expected a `{selector}` send: {:?}", code.instructions())
        };
        *receiver_register
    }

    /// After closure conversion a converted closure writes no register of
    /// another activation, so a send is no longer a write of every local. A
    /// mutable local that no inlinable branch of the body assigns survives
    /// every send lowered after it, and the operand stays in its own
    /// register.
    #[test]
    fn an_operand_is_no_longer_copied_because_a_send_might_write_it() {
        let source = indoc! {"
            class X = Object new (
            ) (
              m = ( | t ::= 1. | ^t + self foo )
            ) : ()
        "};
        let code = lower_whole_method(source, 0); // `t` is register 0
        assert!(
            !code
                .instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Move { source: 0, .. })),
            "nothing the `foo` send runs can write `t`'s register: {:?}",
            code.instructions()
        );
        assert_eq!(
            public_receiver(&code, "+"),
            0,
            "the `+` send reads `t`'s own register"
        );
    }

    /// The exception: a fallback closure writes by reference exactly the
    /// locals its inlinable branch assigns, and it can be kept and run at any
    /// later time, so any send may be the one that runs it.
    #[test]
    fn an_operand_a_branch_assigns_is_still_copied() {
        let source = indoc! {"
            class X = Object new (
            ) (
              m: c = ( | t ::= 1. | c ifTrue: [t:: 9]. ^t + self foo )
            ) : ()
        "};
        let code = lower_whole_method(source, 0); // `c` is register 0, `t` register 1
        let receiver = public_receiver(&code, "+");
        assert_ne!(
            receiver,
            1,
            "the `ifTrue:` fallback closure can write `t` from inside `foo`: {:?}",
            code.instructions()
        );
        assert!(
            code.instructions().contains(&Instruction::Move {
                destination: receiver,
                source: 1
            }),
            "the copy out of `t` is a Move: {:?}",
            code.instructions()
        );
    }

    /// And an assignment lowered between the operand and its consumer writes
    /// the register directly, whatever any closure can reach.
    #[test]
    fn an_operand_a_later_argument_assigns_is_still_copied() {
        let source = indoc! {"
            class X = Object new (
            ) (
              first: x second: y = ( ^x )
              m = ( | t ::= 1. | ^self first: t second: (t:: 9) )
            ) : ()
        "};
        let code = lower_whole_method(source, 1); // `t` is register 0
                                                  // An explicit `self` receiver is a depth-0 lexical send.
        let instructions = code.instructions();
        let Some(Instruction::SendLexical {
            argument_sources, ..
        }) = instructions
            .iter()
            .find(|instruction| matches!(instruction, Instruction::SendLexical { .. }))
        else {
            panic!("expected the keyword send: {:?}", code.instructions());
        };
        assert_ne!(argument_sources[0], 0, "`t` is register 0");
        assert!(
            code.instructions().contains(&Instruction::Move {
                destination: argument_sources[0],
                source: 0
            }),
            "the copy out of `t` is a Move: {:?}",
            code.instructions()
        );
    }

    /// The two nested inlined loops of
    /// `goldens/language/block/writes_two_inlined_levels_deep.ns`, as a method body
    /// and as a block body. Both levels are loops on purpose: a loop's
    /// guard-failure path signals `NonBooleanReceiver`, with no closure, so no
    /// inlinable conditional assigns `t` and `branch_assigned` is false. The
    /// copy can therefore come only from resolving the assignment's target
    /// back to `t`'s own register, which is where `writes_register`'s
    /// `inlined_depth` arithmetic is.
    const TWO_INLINED_LEVELS: &str = "t + ({[outerDone] whileFalse: [
                     [innerDone] whileFalse: [innerDone:: true. t:: 30].
                     outerDone:: true]. 1} at: 2)";

    /// The register the `+` send reads its receiver out of, and the register
    /// the copy out of `t` (register 0) went into.
    fn receiver_and_copy_of_the_binary_send(code: &CompiledCode) -> (u16, u16) {
        let receiver = public_receiver(code, "+");
        let instructions = code.instructions();
        let Some(Instruction::Move { destination, .. }) = instructions
            .iter()
            .find(|instruction| matches!(instruction, Instruction::Move { source: 0, .. }))
        else {
            panic!(
                "expected a copy out of `t`'s register: {:?}",
                code.instructions()
            );
        };
        (receiver, *destination)
    }

    /// In a root body a wrong `inlined_depth` underflows `declaring_scope`'s
    /// `scopes.len() - 1 - depth` and panics, which a release build would
    /// still hit.
    #[test]
    fn an_assignment_two_inlined_levels_deep_in_a_method_copies_the_operand() {
        let source = formatdoc! {"
            class X = Object new (
            ) (
              m = (
                | t ::= 2. outerDone ::= false. innerDone ::= false. |
                ^{TWO_INLINED_LEVELS}
              )
            ) : ()
        "};
        let code = lower_whole_method(&source, 0); // `t` is register 0
        let (receiver, copy) = receiver_and_copy_of_the_binary_send(&code);
        assert_ne!(receiver, 0, "`t` is written before the `+` send reads it");
        assert_eq!(
            receiver,
            copy,
            "the `+` send reads the copy of `t`: {:?}",
            code.instructions()
        );
    }

    /// Inside a block body the scope stack is more than one frame deep, so
    /// an off-by-one does not underflow: it resolves against the enclosing
    /// frame's layout instead and answers no register of this body at all --
    /// a missed copy, and silent wrong output at exit status 0. This is the
    /// case the panic above cannot stand in for.
    #[test]
    fn an_assignment_two_inlined_levels_deep_in_a_block_copies_the_operand() {
        let source = formatdoc! {"
            class X = Object new (
            ) (
              m = (
                ^[ | t ::= 2. outerDone ::= false. innerDone ::= false. |
                   {TWO_INLINED_LEVELS}]
              )
            ) : ()
        "};
        let code = lower_whole_method(&source, 0);
        let instructions = code.instructions();
        let Some(Instruction::MakeClosure { code: block, .. }) = instructions.first() else {
            panic!("expected the block literal: {:?}", code.instructions());
        };
        assert_eq!(
            block.copied_count, 0,
            "the block reads nothing of its method, so `t` is its register 0"
        );
        let (receiver, copy) = receiver_and_copy_of_the_binary_send(block);
        assert_ne!(receiver, 0, "`t` is written before the `+` send reads it");
        assert_eq!(
            receiver,
            copy,
            "the `+` send reads the copy of `t`: {:?}",
            block.instructions()
        );
    }

    #[test]
    fn assigning_a_named_temporary_emits_store_local_and_records_the_statement_range() {
        let (method, bindings) = resolve_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo = ( | t | t:: 7. ^t )
                ) : ()
            "},
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings); // temporary `t` is register 0, base 1
        lowerer.lower_statement(&method.body.statements[0], ValueUse::Discarded); // `t:: 7.`
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 1,
                    literal: BytecodeLiteral::SmallInteger(7)
                },
                Instruction::StoreLocal { source: 1, slot: 0 },
            ]
        );
        assert_eq!(code.register_count, 2);
        assert_eq!(code.statement_program_counter, vec![(0, 0..2)]);
    }

    #[test]
    fn scratch_registers_reset_between_statements() {
        let (method, bindings) = resolve_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo = ( | t | t:: 1. t:: 2. ^t )
                ) : ()
            "},
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings);
        lowerer.lower_statement(&method.body.statements[0], ValueUse::Discarded);
        lowerer.lower_statement(&method.body.statements[1], ValueUse::Discarded);
        let code = lowerer.finish(method.span);
        // Both literals land in scratch register 1 -- the allocator reset
        // between statements instead of growing.
        assert_eq!(
            code.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 1,
                    literal: BytecodeLiteral::SmallInteger(1)
                },
                Instruction::StoreLocal { source: 1, slot: 0 },
                Instruction::LoadLiteral {
                    destination: 1,
                    literal: BytecodeLiteral::SmallInteger(2)
                },
                Instruction::StoreLocal { source: 1, slot: 0 },
            ]
        );
        assert_eq!(code.register_count, 2);
        assert_eq!(code.statement_program_counter, vec![(0, 0..2), (1, 2..4)]);
    }

    #[test]
    fn an_integer_literal_beyond_i64_lowers_to_a_large_integer_literal() {
        let (method, bindings) = resolve_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo = ( ^99999999999999999999 )
                ) : ()
            "},
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings);
        lowerer.lower_expression(return_expression(&method));
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instructions(),
            vec![Instruction::LoadLiteral {
                destination: 0,
                literal: BytecodeLiteral::LargeInteger(
                    crate::ArbitraryInteger::parse("99999999999999999999", 10).unwrap()
                ),
            }]
        );
    }

    #[test]
    fn a_radix_literal_is_parsed_at_compile_time() {
        let (method, bindings) = resolve_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo = ( ^16rFF )
                ) : ()
            "},
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings);
        lowerer.lower_expression(return_expression(&method));
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instructions(),
            vec![Instruction::LoadLiteral {
                destination: 0,
                literal: BytecodeLiteral::SmallInteger(255)
            }]
        );
    }

    #[test]
    fn an_ordinary_send_evaluates_receiver_then_arguments() {
        // `~=` rather than `+`: an integer leaf lowers to the inlined fast
        // path instead, which is a shape of its own
        // (`an_integer_addition_lowers_to_the_inlined_fast_path`).
        let (method, bindings) = resolve_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo: x = ( ^3 ~= x )
                ) : ()
            "},
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings);
        let destination = lowerer.lower_expression(return_expression(&method));
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 1,
                    literal: BytecodeLiteral::SmallInteger(3)
                },
                Instruction::SendPublic {
                    selector: crate::Symbol::intern("~="),
                    receiver_register: 1,
                    argument_base: 2,
                    argument_sources: vec![0],
                    destination: 3,
                    call_site: CallSite { number: 0 },
                },
            ]
        );
        assert_eq!(destination, 3);
        assert_eq!(code.register_count, 4);
        assert_eq!(code.family_size, 1);
    }

    #[test]
    fn a_bare_implicit_send_lowers_to_send_lexical() {
        // `foo`'s body (`^bar`) resolves `bar` as an implicit-receiver unary
        // send at depth 0 (`bar` is a sibling method) -- `Binding::LexicalSend`.
        let source = indoc! {"
            class X = Object new (
            ) (
              bar = ( ^42 )
              foo = ( ^bar )
            ) : ()
        "};
        let (foo, bindings) = resolve_method(source, 1);
        let mut lowerer = method_lowerer(&foo, &bindings);
        let destination = lowerer.lower_expression(return_expression(&foo));
        let code = lowerer.finish(foo.span);
        assert_eq!(
            code.instructions(),
            vec![Instruction::SendLexical {
                depth: 0,
                selector: crate::Symbol::intern("bar"),
                argument_base: 0,
                argument_sources: vec![],
                destination: 0,
                call_site: CallSite { number: 0 },
            }]
        );
        assert_eq!(destination, 0);
    }

    #[test]
    fn a_keyword_implicit_send_and_a_super_send_lower_without_a_receiver_register() {
        let source = indoc! {"
            class X = Object new (
            ) (
              at: i put: v = ( ^v )
              foo = ( at: 1 put: 2. ^super printString )
            ) : ()
        "};
        let (foo, bindings) = resolve_method(source, 1);
        let mut lowerer = method_lowerer(&foo, &bindings);
        lowerer.lower_statement(&foo.body.statements[0], ValueUse::Discarded);
        let code = lowerer.finish(foo.span);
        assert!(matches!(
            &code.instruction(2),
            Instruction::SendLexical { depth: 0, selector, argument_sources, .. }
                if selector.as_str() == "at:put:" && argument_sources.len() == 2
        ));
        let (foo, bindings) = resolve_method(source, 1);
        let mut lowerer = method_lowerer(&foo, &bindings);
        lowerer.lower_expression(match &foo.body.statements[1].kind {
            StatementKind::Return(expression) => expression,
            _ => panic!(),
        });
        let code = lowerer.finish(foo.span);
        assert!(matches!(
            &code.instruction(0),
            Instruction::SendSuper { selector, argument_sources, destination: 0, .. }
                if selector.as_str() == "printString" && argument_sources.is_empty()
        ));
    }

    #[test]
    fn a_slot_assignment_lowers_to_a_setter_lexical_send() {
        let source = indoc! {"
            class X = Object new (
              | n ::= 0. |
            ) (
              bump = ( n:: n + 1 )
            ) : ()
        "};
        let (bump, bindings) = resolve_method(source, 0);
        let mut lowerer = method_lowerer(&bump, &bindings);
        lowerer.lower_statement(&bump.body.statements[0], ValueUse::Discarded);
        let code = lowerer.finish(bump.span);
        assert!(matches!(
            code.instructions().last(),
            Some(Instruction::SendLexical { depth: 0, selector, argument_sources, .. })
                if selector.as_str() == "n:" && argument_sources.len() == 1
        ));
    }

    /// The argument register of every `n:` setter send in `code`, in order.
    fn setter_arguments(code: &CompiledCode) -> Vec<u16> {
        code.instructions()
            .iter()
            .filter_map(|instruction| match instruction {
                Instruction::SendLexical {
                    selector,
                    argument_sources,
                    ..
                } if selector.as_str() == "n:" => Some(argument_sources[0]),
                _ => None,
            })
            .collect()
    }

    /// The copy `lower_assign` makes for a setter send protects the
    /// assignment's *value* from the user code that setter runs, which can
    /// write the local before anything reads the value -- even a consumer
    /// that is the very next instruction. Only where nothing reads the value
    /// is the copy pure overhead: one instruction, and one more register in
    /// every activation's `temporaries`. This test pins the cheap form for a
    /// statement; the tests after it pin the copy for each position that
    /// reads the value, and `ns-cli`'s
    /// `an_assignment_through_a_setter_answers_the_value_before_the_setter_ran`
    /// pins what a program observes.
    ///
    /// `true ifTrue: [t:: 2]` is what makes the copy possible at all after
    /// closure conversion: it is the fallback closure of an inlined
    /// conditional that writes `t`, by reference, from inside the setter
    /// (`docs/design/execution.md`, "Fewer protective copies"). Without it
    /// neither this test nor the next would copy, and the pair would not
    /// contrast.
    #[test]
    fn a_slot_assignment_in_statement_position_does_not_copy_its_value() {
        let source = indoc! {"
            class X = Object new (
              | n ::= 0. |
            ) (
              m = ( | t | t:: 1. true ifTrue: [t:: 2]. n:: t )
            ) : ()
        "};
        let code = lower_whole_method(source, 0); // temporary `t` is register 0
        assert!(
            !code
                .instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Move { source: 0, .. })),
            "nothing reads the assignment's value, so it needs no copy: {:?}",
            code.instructions()
        );
        assert_eq!(
            setter_arguments(&code),
            vec![0],
            "the setter takes `t`'s own register"
        );
    }

    #[test]
    fn a_returned_slot_assignment_copies_its_value_out_of_the_local() {
        let source = indoc! {"
            class X = Object new (
              | n ::= 0. |
            ) (
              m = ( | t | t:: 1. true ifTrue: [t:: 2]. ^n:: t )
            ) : ()
        "};
        let code = lower_whole_method(source, 0); // temporary `t` is register 0
        let [copy] = setter_arguments(&code)[..] else {
            panic!("expected one setter send: {:?}", code.instructions());
        };
        assert_ne!(copy, 0, "the setter can write `t` before `Return` reads");
        assert!(code.instructions().contains(&Instruction::Move {
            destination: copy,
            source: 0
        }));
        assert_eq!(
            code.instructions().last(),
            Some(&Instruction::Return { register: copy })
        );
    }

    #[test]
    fn a_slot_initializer_that_assigns_a_slot_copies_its_value_out_of_the_local() {
        // A method body stands in for the initializer expressions, as in
        // `a_slot_initializer_is_one_statement_ending_in_initialize_slot`; the
        // mutable temporary gives the assignment a named local to hand on --
        // a parameter would need no copy at all
        // (`a_slot_initializer_that_assigns_a_parameter_copies_nothing`).
        // Lowering an `Initializer` from a method's own temporaries is an
        // artificial shape -- a real initializer has no locals of its own --
        // used here only to pin `materialize`'s copy mechanism generically,
        // independent of `CodeKind`. The first initializer is what makes `p`
        // one a fallback closure can write, and so the copy possible at all
        // (`a_slot_assignment_in_statement_position_does_not_copy_its_value`).
        let source = indoc! {"
            class X = Object new (
              | n ::= 0. |
            ) (
              foo: c = ( | p | c ifTrue: [p:: 1]. ^n:: p )
            ) : ()
        "};
        let (method, bindings) = resolve_method(source, 0);
        let initializers: Vec<&Expression> = method
            .body
            .statements
            .iter()
            .map(|statement| match &statement.kind {
                StatementKind::Expression(expression) | StatementKind::Return(expression) => {
                    expression
                }
            })
            .collect();
        let mut lowerer = lowerer_for(
            CodeKind::Initializer,
            1, // `c` is the factory parameter, register 0
            &method.body.temporaries,
            initializers.clone(),
            &[],
            &bindings,
        );
        lowerer.lower_slot_initializer(initializers[0], 3);
        lowerer.lower_slot_initializer(initializers[1], 4);
        let code = lowerer.finish(method.span);
        let [copy] = setter_arguments(&code)[..] else {
            panic!("expected one setter send: {:?}", code.instructions());
        };
        assert_ne!(
            copy, 1,
            "the setter can write `p` before `InitializeSlot` reads"
        );
        assert!(
            code.instructions().contains(&Instruction::Move {
                destination: copy,
                source: 1
            }),
            "the copy out of `p` is a Move: {:?}",
            code.instructions()
        );
        assert_eq!(
            code.instructions().last(),
            Some(&Instruction::InitializeSlot {
                source: copy,
                own_index: 4
            })
        );
    }

    /// A parameter needs no copy even here: nothing but its own
    /// initialization can write it (`a_parameter_operand_is_never_copied`),
    /// so the setter send reads it straight out of its own register.
    #[test]
    fn a_slot_initializer_that_assigns_a_parameter_copies_nothing() {
        let source = indoc! {"
            class X = Object new (
              | n ::= 0. |
            ) (
              foo: p = ( ^n:: p )
            ) : ()
        "};
        let (method, bindings) = resolve_method(source, 0);
        let mut lowerer = lowerer_for(
            CodeKind::Initializer,
            1,
            &[],
            vec![return_expression(&method)],
            &[],
            &bindings,
        );
        lowerer.lower_slot_initializer(return_expression(&method), 4);
        let code = lowerer.finish(method.span);
        assert!(
            !code
                .instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Move { .. })),
            "a parameter needs no copy: {:?}",
            code.instructions()
        );
        assert_eq!(
            code.instructions(),
            vec![
                Instruction::SendLexical {
                    depth: 0,
                    selector: crate::Symbol::intern("n:"),
                    argument_base: 1,
                    argument_sources: vec![0],
                    destination: 2,
                    call_site: CallSite { number: 0 },
                },
                Instruction::InitializeSlot {
                    source: 0,
                    own_index: 4
                },
            ]
        );
    }

    /// A block answers its last statement's value, so that statement's
    /// assignment is copied; an earlier statement's is not. `t` is a mutable
    /// block-local temporary, not a parameter: a parameter would need no
    /// copy at all (`a_parameter_operand_is_never_copied`). The block's
    /// `true ifTrue: [t:: 1]` is what makes `t` one a fallback closure can
    /// write, and so the copy possible at all
    /// (`a_slot_assignment_in_statement_position_does_not_copy_its_value`);
    /// its receiver is a literal so that the block captures nothing and `t`
    /// stays the block's register 0.
    #[test]
    fn a_blocks_last_slot_assignment_copies_its_value_and_an_earlier_one_does_not() {
        let source = indoc! {"
            class X = Object new (
              | n ::= 0. |
            ) (
              m = ( ^[ | t | true ifTrue: [t:: 1]. n:: t. n:: t] )
            ) : ()
        "};
        let code = lower_whole_method(source, 0);
        let Instruction::MakeClosure { code: block, .. } = &code.instruction(0) else {
            panic!("expected the block literal: {:?}", code.instructions());
        };
        let [discarded, read] = setter_arguments(block)[..] else {
            panic!("expected two setter sends: {:?}", block.instructions());
        };
        assert_eq!(
            discarded, 0,
            "the first value is discarded: `t`'s own register"
        );
        assert_ne!(read, 0, "the block answers the second value");
        assert!(block.instructions().contains(&Instruction::Move {
            destination: read,
            source: 0
        }));
        assert_eq!(
            block.instructions().last(),
            Some(&Instruction::ImplicitReturn { register: read })
        );
    }

    /// A block parameter is read-only exactly like a method parameter
    /// (`a_parameter_operand_is_never_copied`): `lower_block_literal` builds
    /// its own `Lowerer` with its own `read_only_locals`, so the second
    /// setter send here reads `t` straight out of its own register too.
    #[test]
    fn a_block_parameter_operand_is_never_copied() {
        let source = indoc! {"
            class X = Object new (
              | n ::= 0. |
            ) (
              m = ( ^[:t | n:: t. n:: t] )
            ) : ()
        "};
        let code = lower_whole_method(source, 0);
        let Instruction::MakeClosure { code: block, .. } = &code.instruction(0) else {
            panic!("expected the block literal: {:?}", code.instructions());
        };
        assert!(
            !block
                .instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Move { source: 0, .. })),
            "a block parameter needs no copy: {:?}",
            block.instructions()
        );
        assert_eq!(
            setter_arguments(block),
            vec![0, 0],
            "both setter sends read `t`'s own register directly"
        );
    }

    /// An inlined block's last statement is read exactly when the inlined
    /// form's own value is -- a loop condition always, a loop body never.
    /// The leading `true ifTrue: [t:: 1]` is what makes `t` one a fallback
    /// closure can write, and so the copy possible at all
    /// (`a_slot_assignment_in_statement_position_does_not_copy_its_value`).
    #[test]
    fn an_inlined_blocks_last_slot_assignment_is_copied_only_when_its_value_is_read() {
        let source = indoc! {"
            class X = Object new (
              | n ::= 0. |
            ) (
              discarded: c = ( | t | true ifTrue: [t:: 1]. c ifTrue: [n:: t] ifFalse: [n:: t]. c and: [n:: t] )
              returned: c = ( | t | true ifTrue: [t:: 1]. ^c ifTrue: [n:: t] ifFalse: [n:: t] )
              returnedAnd: c = ( | t | true ifTrue: [t:: 1]. ^c and: [n:: t] )
              loop = ( | t | true ifTrue: [t:: 1]. [n:: t] whileTrue: [n:: t] )
            ) : ()
        "};
        // `c` is register 0 and `t` register 1; `loop`'s `t` is register 0.
        let discarded = lower_whole_method(source, 0);
        assert_eq!(setter_arguments(&discarded), vec![1, 1, 1]);
        for (index, count) in [(1, 2), (2, 1)] {
            let returned = lower_whole_method(source, index);
            let arguments = setter_arguments(&returned);
            assert_eq!(arguments.len(), count);
            assert!(
                arguments.iter().all(|&argument| argument > 1),
                "the send's value is read, so each branch's is: {:?}",
                returned.instructions()
            );
        }
        let loop_code = lower_whole_method(source, 3);
        let [condition, body] = setter_arguments(&loop_code)[..] else {
            panic!("expected two setter sends: {:?}", loop_code.instructions());
        };
        assert_ne!(condition, 0, "the guard reads the condition's value");
        assert_eq!(body, 0, "nothing reads the loop body's value");
    }

    /// §5.7: a name no enclosing declaration declares is `self
    /// Array`, which `Object`'s accessor answers (§3.2) -- a
    /// depth-0 lexical send, never a constant load.
    #[test]
    fn a_bare_builtin_class_name_lowers_to_a_self_send() {
        let source = indoc! {"
            class X = Object new (
            ) (
              foo = ( ^Array )
            ) : ()
        "};
        let (foo, bindings) = resolve_method(source, 0);
        let mut lowerer = method_lowerer(&foo, &bindings);
        let destination = lowerer.lower_expression(return_expression(&foo));
        let code = lowerer.finish(foo.span);
        let [Instruction::SendLexical {
            depth: 0,
            selector,
            argument_sources,
            destination: send_destination,
            ..
        }] = &code.instructions()[..]
        else {
            panic!(
                "expected one depth-0 lexical send: {:?}",
                code.instructions()
            );
        };
        assert_eq!(selector.as_str(), "Array");
        assert!(argument_sources.is_empty());
        assert_eq!(*send_destination, destination);
    }

    #[test]
    fn a_cascade_reuses_one_receiver_register_across_every_message() {
        let source = indoc! {"
            class X = Object new (
            ) (
              foo = ( | c | c:: List new. c add: 1; add: 2 )
            ) : ()
        "};
        let (foo, bindings) = resolve_method(source, 0);
        let mut lowerer = method_lowerer(&foo, &bindings); // temporary `c` is register 0
        lowerer.lower_statement(&foo.body.statements[0], ValueUse::Discarded);
        lowerer.lower_statement(&foo.body.statements[1], ValueUse::Discarded);
        let code = lowerer.finish(foo.span);
        let receiver_registers: Vec<u16> = code
            .instructions()
            .iter()
            .filter_map(|instruction| match instruction {
                Instruction::SendPublic {
                    receiver_register, ..
                } => Some(*receiver_register),
                _ => None,
            })
            .collect();
        // `List new`, then the cascade's two `add:` sends.
        assert_eq!(receiver_registers.len(), 3);
        // Nothing the `add:` sends run can write `c`'s register -- no
        // inlinable branch of this body assigns it, so no fallback closure
        // reaches it -- so the cascade reads `c` itself.
        assert_eq!(
            receiver_registers[1],
            0,
            "the cascade receiver is `c`'s own register: {:?}",
            code.instructions()
        );
        assert_eq!(
            receiver_registers[1], receiver_registers[2],
            "both cascade sends share one receiver register"
        );
        // Four call sites: `self List` (§5.7), `new` and the two
        // `add:` sends.
        assert_eq!(code.family_size, 4);
    }

    /// The registers the cascade's `add:` sends read their receiver out of.
    fn cascade_receivers(code: &CompiledCode) -> (u16, u16) {
        let receiver_registers: Vec<u16> = code
            .instructions()
            .iter()
            .filter_map(|instruction| match instruction {
                Instruction::SendPublic {
                    selector,
                    receiver_register,
                    ..
                } if selector.as_str() == "add:" => Some(*receiver_register),
                _ => None,
            })
            .collect();
        let [first, second] = receiver_registers[..] else {
            panic!(
                "expected the cascade's two sends: {:?}",
                code.instructions()
            );
        };
        (first, second)
    }

    /// When a fallback closure *can* write the receiver's local, the sends
    /// between the cascade's messages can run it, so the receiver is copied
    /// out first and every message still shares that one register.
    #[test]
    fn a_cascade_receiver_a_branch_assigns_is_copied_out_of_its_local() {
        let source = indoc! {"
            class X = Object new (
            ) (
              foo = (
                | c |
                c:: List new.
                true ifTrue: [c:: List new].
                c add: 1; add: 2
              )
            ) : ()
        "};
        let code = lower_whole_method(source, 0); // temporary `c` is register 0
        let (first, second) = cascade_receivers(&code);
        assert_ne!(first, 0, "the receiver must not be `c`'s own live register");
        assert_eq!(first, second, "both cascade sends share one register");
        assert!(
            code.instructions().contains(&Instruction::Move {
                destination: first,
                source: 0
            }),
            "the copy out of `c` is a Move: {:?}",
            code.instructions()
        );
    }

    /// And the arguments of *every* message are lowered before the last send
    /// reads the receiver, not only the first's -- so an assignment in a
    /// later message's argument copies the receiver out just as one in the
    /// first message's does.
    #[test]
    fn a_cascade_receiver_a_later_messages_argument_assigns_is_copied_out_of_its_local() {
        let source = indoc! {"
            class X = Object new (
            ) (
              foo = (
                | c |
                c:: List new.
                c add: 1; add: (c:: List new)
              )
            ) : ()
        "};
        let code = lower_whole_method(source, 0); // temporary `c` is register 0
        let (first, second) = cascade_receivers(&code);
        assert_ne!(first, 0, "the receiver must not be `c`'s own live register");
        assert_eq!(first, second, "both cascade sends share one register");
        assert!(
            code.instructions().contains(&Instruction::Move {
                destination: first,
                source: 0
            }),
            "the copy out of `c` is a Move: {:?}",
            code.instructions()
        );
    }

    #[test]
    fn hops_count_only_frame_scopes_within_the_depth() {
        let frame = ScopeKind::Frame {
            key: BodyKey::Root,
            by_reference: false,
        };
        let inlined = ScopeKind::Inlined;
        // [Frame, Inlined, Frame] innermost-last: from the innermost frame,
        // resolver depth 2 (block -> inlined -> method) names scope 0, and
        // one frame stands above it.
        assert_eq!(hops_above(&[frame, inlined, frame], 0), 1);
        // From directly inside an inlined block, depth 1 is the enclosing
        // frame's own registers: zero hops.
        assert_eq!(hops_above(&[frame, inlined], 0), 0);
        // No inlining anywhere: hops == depth.
        assert_eq!(hops_above(&[frame, frame, frame], 0), 2);
        assert_eq!(hops_above(&[frame], 0), 0);
    }

    fn lower_whole_method(source: &str, index: usize) -> CompiledCode {
        let (method, bindings) = resolve_method(source, index);
        let mut lowerer = method_lowerer(&method, &bindings);
        lowerer.lower_local_initializers(&method.body.temporaries);
        lowerer.lower_body(&method.body.statements);
        lowerer.finish(method.span)
    }

    #[test]
    fn a_method_with_no_explicit_return_implicitly_returns_self() {
        let code = lower_whole_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo = ( 1 + 1 )
                ) : ()
            "},
            0,
        );
        assert_eq!(
            &code.instructions()[code.instructions().len() - 2..],
            &[
                Instruction::LoadSelf { destination: 3 },
                Instruction::ImplicitReturn { register: 3 }
            ]
        );
    }

    #[test]
    fn an_explicit_method_return_is_a_plain_return_with_nothing_appended() {
        let code = lower_whole_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo = ( ^1 )
                ) : ()
            "},
            0,
        );
        assert_eq!(
            code.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 0,
                    literal: BytecodeLiteral::SmallInteger(1)
                },
                Instruction::Return { register: 0 },
            ]
        );
    }

    #[test]
    fn a_block_literal_compiles_to_its_own_code_carried_by_make_closure() {
        let code = lower_whole_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo = ( ^[ :x | x ] )
                ) : ()
            "},
            0,
        );
        let instructions = code.instructions();
        let Some(Instruction::MakeClosure {
            destination: 0,
            parameters,
            code: block,
            copied,
        }) = instructions.first()
        else {
            panic!(
                "expected a MakeClosure instruction, got {:?}",
                code.instructions()
            );
        };
        assert_eq!(parameters.as_slice(), &[crate::Symbol::intern("x")]);
        assert!(
            copied.is_empty(),
            "the block captures nothing, so it is handed nothing"
        );
        assert_eq!(block.kind, CodeKind::Block);
        assert_eq!(block.argument_count, 1);
        // `x` is the block's own register 0: read with no instruction, then
        // returned as the block's implicit value.
        assert_eq!(
            block.instructions(),
            vec![Instruction::ImplicitReturn { register: 0 }]
        );
        assert_eq!(block.statement_program_counter, vec![(0, 0..0)]);
    }

    /// A family's sends are numbered densely in the order they are emitted,
    /// blocks included, and every body records one past the highest number in
    /// itself and the blocks nested in it (`docs/design/dispatch.md`, "Code
    /// families").
    #[test]
    fn a_family_numbers_its_sends_densely_across_nested_blocks() {
        let source = indoc! {"
            class Subject = Object new (
            ) (
              public run: items = (
                items size.
                items do: [:each | each printString. [:inner | inner foo] value: each].
                ^items isEmpty
              )
            ) : ()
        "};
        let code = lower_whole_method(source, 0);
        let numbers = |code: &CompiledCode| -> Vec<u32> {
            code.instructions()
                .iter()
                .filter_map(Instruction::call_site)
                .map(|site| site.number)
                .collect()
        };
        let first_block = |code: &CompiledCode| -> Rc<CompiledCode> {
            code.instructions()
                .iter()
                .find_map(|instruction| match instruction {
                    Instruction::MakeClosure { code: block, .. } => Some(block.clone()),
                    _ => None,
                })
                .expect("the body creates a block")
        };
        // Emission order: `size` 0; the `do:` block's `printString` 1; the inner
        // block's `foo` 2, lowered when its literal is reached; `value:` 3;
        // `do:` 4, after its block literal; `isEmpty` 5. Verified against
        // `ns-next-analyzer disasm` on START when the plan was written.
        assert_eq!(numbers(&code), vec![0, 4, 5]);
        assert_eq!(code.family_size, 6);
        let block = first_block(&code);
        assert_eq!(numbers(&block), vec![1, 3]);
        assert_eq!(block.family_size, 4);
        let inner = first_block(&block);
        assert_eq!(numbers(&inner), vec![2]);
        assert_eq!(inner.family_size, 3);
    }

    /// The same source as the by-reference model's own test: the block reads
    /// `x` out of its own copied register and writes `t` through the vector
    /// the method built for it, with no frame hop at all.
    #[test]
    fn a_block_reads_a_copied_value_and_writes_through_its_scopes_vector() {
        let code = lower_whole_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo: x = ( | t | ^[ t:: x ] )
                ) : ()
            "},
            0,
        );
        // `x` is r0, `t` is r1 (unused: it lives in the vector), the vector
        // is r2.
        assert_eq!(
            code.instruction(0),
            Instruction::MakeVector {
                destination: 2,
                size: 1
            }
        );
        let instructions = code.instructions();
        let Some(Instruction::MakeClosure {
            code: block,
            copied,
            ..
        }) = instructions.get(1)
        else {
            panic!()
        };
        // The parameter's own register, then the vector register.
        assert_eq!(*copied, vec![0, 2]);
        assert_eq!(block.copied_count, 2);
        assert_eq!(block.outer_reach, 0);
        assert_eq!(
            block.instructions(),
            vec![
                Instruction::StoreVector {
                    vector: 1,
                    index: 0,
                    source: 0
                }, // t:: x
                Instruction::ImplicitReturn { register: 0 },
            ]
        );
    }

    #[test]
    fn a_block_reads_a_copied_variable_as_its_own_register() {
        let code = lower_whole_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo: x = ( ^[x] )
                ) : ()
            "},
            0,
        );
        let instructions = code.instructions();
        let Some(Instruction::MakeClosure {
            code: block,
            copied,
            ..
        }) = instructions.first()
        else {
            panic!()
        };
        assert_eq!(*copied, vec![0]); // the parameter's own register
        assert_eq!(block.copied_count, 1);
        assert_eq!(
            block.instructions(),
            vec![Instruction::ImplicitReturn { register: 0 }]
        );
        assert_eq!(block.outer_reach, 0);
    }

    #[test]
    fn a_block_that_writes_shares_its_scopes_vector() {
        let code = lower_whole_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo = ( | t | ^[t:: 1] )
                ) : ()
            "},
            0,
        );
        assert_eq!(
            code.instruction(0),
            Instruction::MakeVector {
                destination: 1,
                size: 1
            }
        );
        let instructions = code.instructions();
        let Some(Instruction::MakeClosure {
            code: block,
            copied,
            ..
        }) = instructions.get(1)
        else {
            panic!()
        };
        assert_eq!(*copied, vec![1]); // the vector register
        assert_eq!(
            block.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 1,
                    literal: BytecodeLiteral::SmallInteger(1)
                },
                Instruction::StoreVector {
                    vector: 0,
                    index: 0,
                    source: 1
                },
                Instruction::ImplicitReturn { register: 1 },
            ]
        );
    }

    #[test]
    fn a_doubly_nested_block_is_handed_the_variable_by_the_block_between() {
        let code = lower_whole_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo: x = ( ^[[x]] )
                ) : ()
            "},
            0,
        );
        let instructions = code.instructions();
        let Some(Instruction::MakeClosure {
            code: outer,
            copied,
            ..
        }) = instructions.first()
        else {
            panic!()
        };
        assert_eq!(*copied, vec![0]);
        let instructions = outer.instructions();
        let Some(Instruction::MakeClosure {
            code: inner,
            copied: inner_copied,
            ..
        }) = instructions.first()
        else {
            panic!()
        };
        // The middle block holds it in its own first copied register.
        assert_eq!(*inner_copied, vec![0]);
        assert_eq!(
            inner.instructions(),
            vec![Instruction::ImplicitReturn { register: 0 }]
        );
    }

    #[test]
    fn the_fallback_closure_of_an_inlined_conditional_stays_by_reference() {
        let code = lower_whole_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo: c = ( | t | c ifTrue: [t:: 1]. ^t )
                ) : ()
            "},
            0,
        );
        let fallback = code
            .instructions()
            .iter()
            .find_map(|instruction| match instruction {
                Instruction::MakeClosure {
                    code: block,
                    copied,
                    ..
                } => {
                    assert!(copied.is_empty());
                    Some(block.clone())
                }
                _ => None,
            })
            .expect("the fallback path builds a real closure");
        assert!(fallback.instructions().iter().any(|instruction| matches!(
            instruction,
            Instruction::StoreOuterLocal {
                depth: 1,
                index: 1,
                ..
            }
        )));
        assert_eq!(fallback.outer_reach, 1);
        // Nothing captured `t`, so it stays a register.
        assert!(!code
            .instructions()
            .iter()
            .any(|instruction| matches!(instruction, Instruction::MakeVector { .. })));
    }

    #[test]
    fn a_return_inside_a_block_is_a_non_local_return() {
        let code = lower_whole_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo = ( ^[ ^1 ] )
                ) : ()
            "},
            0,
        );
        let instructions = code.instructions();
        let Some(Instruction::MakeClosure { code: block, .. }) = instructions.first() else {
            panic!()
        };
        assert_eq!(
            block.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 0,
                    literal: BytecodeLiteral::SmallInteger(1)
                },
                Instruction::NonLocalReturn { register: 0 },
            ]
        );
    }

    #[test]
    fn an_empty_block_returns_nil_and_a_multi_statement_block_returns_its_last_value() {
        let code = lower_whole_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo = ( [ ] value. ^[ 1. 2 ] value )
                ) : ()
            "},
            0,
        );
        let instructions = code.instructions();
        let Some(Instruction::MakeClosure { code: empty, .. }) = instructions.first() else {
            panic!()
        };
        assert_eq!(
            empty.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 0,
                    literal: BytecodeLiteral::Nil
                },
                Instruction::ImplicitReturn { register: 0 }
            ]
        );
        let instructions = code.instructions();
        let Some(Instruction::MakeClosure { code: two, .. }) = instructions.get(2) else {
            panic!("{:?}", code.instructions())
        };
        assert_eq!(
            two.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 0,
                    literal: BytecodeLiteral::SmallInteger(1)
                },
                Instruction::LoadLiteral {
                    destination: 0,
                    literal: BytecodeLiteral::SmallInteger(2)
                },
                Instruction::ImplicitReturn { register: 0 },
            ]
        );
    }

    #[test]
    fn an_empty_inlined_block_yields_nil() {
        // A literally empty inlined block: `lower_inline_block_body`'s "no
        // statements" path (`result` stays `None`, falling back to
        // `emit_nil`), which no other test reaches directly.
        let code = compile_foo("x ifTrue: [] ifFalse: [2]");
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::LoadLiteral {
                literal: BytecodeLiteral::Nil,
                ..
            }
        )));
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::LoadLiteral {
                literal: BytecodeLiteral::SmallInteger(2),
                ..
            }
        )));
    }

    #[test]
    fn a_dynamic_array_literal_lowers_to_make_array() {
        let (method, bindings) = resolve_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo = ( ^{ 1. 2. 3 } )
                ) : ()
            "},
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings);
        let destination = lowerer.lower_expression(return_expression(&method));
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 0,
                    literal: BytecodeLiteral::SmallInteger(1)
                },
                Instruction::LoadLiteral {
                    destination: 1,
                    literal: BytecodeLiteral::SmallInteger(2)
                },
                Instruction::LoadLiteral {
                    destination: 2,
                    literal: BytecodeLiteral::SmallInteger(3)
                },
                Instruction::MakeArray {
                    destination: 3,
                    element_registers: vec![0, 1, 2]
                },
            ]
        );
        assert_eq!(destination, 3);
    }

    #[test]
    fn an_initializer_discards_a_top_level_return_and_always_returns_self() {
        let (method, bindings) = resolve_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo = ( ^1 )
                ) : ()
            "},
            0,
        );
        // Reuse `foo`'s `^1` as if it were an initializer expression: kind Initializer, 0 arguments.
        let mut lowerer = lowerer_for(
            CodeKind::Initializer,
            0,
            &[],
            Vec::new(),
            &method.body.statements,
            &bindings,
        );
        lowerer.lower_body(&method.body.statements);
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 0,
                    literal: BytecodeLiteral::SmallInteger(1)
                },
                Instruction::LoadSelf { destination: 1 },
                Instruction::ImplicitReturn { register: 1 },
            ]
        );
    }

    #[test]
    fn a_slot_initializer_is_one_statement_ending_in_initialize_slot() {
        let (method, bindings) = resolve_method(
            indoc! {"
                class X = Object new (
                ) (
                  foo = ( ^6 * 7 )
                ) : ()
            "},
            0,
        );
        let initializer = return_expression(&method);
        let mut lowerer = lowerer_for(
            CodeKind::Initializer,
            0,
            &[],
            vec![initializer],
            &[],
            &bindings,
        );
        lowerer.lower_slot_initializer(initializer, 4);
        let code = lowerer.finish(method.span);
        // `6`, `7`, the inlined `*` with its guard and its fallback send,
        // then the field write: six instructions, all one statement.
        assert!(matches!(
            code.instructions().last(),
            Some(Instruction::InitializeSlot {
                source: 2,
                own_index: 4
            })
        ));
        assert_eq!(code.statement_program_counter, vec![(0, 0..6)]);
    }

    /// Compiles `foo: x = ( ^<body> )` and returns the method's code.
    fn compile_foo(body: &str) -> CompiledCode {
        let full = formatdoc! {"
            class X = Object new (
            ) (
              foo: x = ( ^{body} )
            ) : ()
        "};
        lower_whole_method(&full, 0)
    }

    fn has<F: Fn(&Instruction) -> bool>(code: &CompiledCode, predicate: F) -> bool {
        code.instructions().iter().any(predicate)
    }

    /// Lowers `method` of a nested class `C` in a kernel-shaped module that
    /// `resolve_kernel` resolved, so that its sends to `vmMirror` carry
    /// `Binding::MirrorSend`.
    fn lower_kernel_method(method: &str) -> CompiledCode {
        let source = formatdoc! {"
            class K vmMirror: theMirror = Object new (
              | private vmMirror = theMirror. |
            ) (
              class C = Object new () (
                {method}
              )
            ) : ()
        "};
        let module = ns_syntax::parse(&source).expect("parse");
        let Ok(resolved) = crate::resolver::resolve_kernel(
            module,
            Rc::from(source.as_str()),
            crate::SourceOrigin::File(PathBuf::from("t.ns")),
        ) else {
            panic!("the kernel-shaped module resolves");
        };
        let method = resolved
            .module
            .root_class
            .instance_side
            .nested_classes()
            .next()
            .expect("a nested class")
            .instance_side
            .methods()
            .next()
            .expect("a method");
        let mut lowerer = method_lowerer(method, &resolved.bindings);
        lowerer.lower_local_initializers(&method.body.temporaries);
        lowerer.lower_body(&method.body.statements);
        lowerer.finish(method.span)
    }

    /// The failure block is code at `failure`, not a closure: the send
    /// evaluates the mirror, the subject and the leaf's argument, then either
    /// jumps over the block or runs it in this body, where its `^` returns
    /// from the method.
    #[test]
    fn a_mirror_send_lowers_its_failure_block_inline() {
        let code = lower_kernel_method(
            "foo: argument = ( ^vmMirror string: self concatenate: argument ifFail: [^nil] )",
        );
        let primitive =
            crate::builtins::primitive::mirror_primitive_named("string:concatenate:ifFail:")
                .expect("the mirror has string:concatenate:ifFail:");
        let expected = vec![
            Instruction::SendLexical {
                depth: 1,
                selector: crate::Symbol::intern("vmMirror"),
                argument_base: 1,
                argument_sources: vec![],
                destination: 1,
                call_site: CallSite { number: 0 },
            },
            Instruction::LoadSelf { destination: 2 },
            // The leaf's argument is the parameter, which the send copies
            // into its slot, r3.
            Instruction::SendMirror {
                primitive,
                mirror_register: 1,
                subject_register: 2,
                argument_base: 3,
                argument_sources: vec![0],
                destination: 4,
                failure: 4,
            },
            Instruction::Jump { target: 8 },
            Instruction::LoadLiteral {
                destination: 5,
                literal: BytecodeLiteral::Nil,
            },
            Instruction::Return { register: 5 },
            Instruction::LoadLiteral {
                destination: 6,
                literal: BytecodeLiteral::Nil,
            },
            Instruction::Move {
                destination: 4,
                source: 6,
            },
            Instruction::Return { register: 4 },
        ];
        assert_eq!(code.instructions(), expected, "{:#?}", code.instructions());
        // The one call site is `vmMirror`'s lexical send: `SendMirror`
        // carries none, since nothing about it is looked up at run time.
        assert_eq!(code.family_size, 1);
        assert_eq!(code.outer_reach, 0);
    }

    /// The instance initializer and a lazy slot's getter run against the
    /// level's own slots; an ordinary method does not.
    #[test]
    fn an_initializer_and_a_lazy_getter_address_their_own_slots_and_a_method_does_not() {
        let source = indoc! {"
            class X = Object new (
              | a = 1. |
            ) (
              lazy b = 2.

              m = ( ^a )
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(source),
            crate::SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = crate::Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        let mixin = interpreter.enter(|mutator| {
            let crate::ValueKind::Object(object) = mutator.fetch(&class).kind() else {
                panic!("expected a class object");
            };
            mutator
                .heap()
                .class_application(object)
                .expect("a class")
                .mixin
                .clone()
        });
        let initializer = mixin.initializer_code.as_ref().expect("an initializer");
        assert!(initializer.addresses_own_slots);
        let flag_of = |selector: &str| {
            mixin
                .compiled_methods()
                .find(|method| method.selector.as_str() == selector)
                .unwrap_or_else(|| panic!("a method {selector}"))
                .code
                .addresses_own_slots
        };
        assert!(flag_of("b"), "the lazy getter");
        assert!(!flag_of("m"));
    }

    /// A failure argument that is not a literal block is an ordinary send
    /// to the mirror, under the optimized lowering as under the reference.
    #[test]
    fn a_mirror_send_whose_failure_argument_is_not_a_block_is_an_ordinary_send() {
        let code =
            lower_kernel_method("foo: argument = ( ^vmMirror stringSize: self ifFail: argument )");
        assert!(!has(&code, |instruction| matches!(
            instruction,
            Instruction::SendMirror { .. }
        )));
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::SendPublic { selector, .. } if selector.as_str() == "stringSize:ifFail:"
        )));
    }

    #[test]
    fn if_true_if_false_with_literal_blocks_compiles_to_a_guarded_branch_with_a_fallback_send() {
        let code = compile_foo("x ifTrue: [1] ifFalse: [2]");
        assert!(
            has(&code, |instruction| matches!(
                instruction,
                Instruction::JumpIfNotBoolean { register: 0, .. }
            )),
            "{:#?}",
            code.instructions()
        );
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::BranchIfFalse { register: 0, .. }
        )));
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::LoadLiteral {
                literal: BytecodeLiteral::SmallInteger(1),
                ..
            }
        )));
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::LoadLiteral {
                literal: BytecodeLiteral::SmallInteger(2),
                ..
            }
        )));
        // The fallback path is a real send of the original selector with real closures.
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::SendPublic { selector, receiver_register: 0, .. } if selector.as_str() == "ifTrue:ifFalse:"
        )));
        assert_eq!(
            code.instructions()
                .iter()
                .filter(|instruction| matches!(instruction, Instruction::MakeClosure { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn a_non_literal_block_argument_is_an_ordinary_send_with_no_guard() {
        let code = compile_foo("x ifTrue: x ifFalse: [2]");
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::SendPublic { selector, .. } if selector.as_str() == "ifTrue:ifFalse:"
        )));
        assert!(!has(&code, |instruction| matches!(
            instruction,
            Instruction::JumpIfNotBoolean { .. }
        )));
    }

    #[test]
    fn a_block_with_temporaries_is_not_inlined() {
        let code = compile_foo("x ifTrue: [ | t | t:: 1. t ] ifFalse: [2]");
        assert!(!has(&code, |instruction| matches!(
            instruction,
            Instruction::JumpIfNotBoolean { .. }
        )));
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::MakeClosure { code, .. } if code.register_count - code.argument_count >= 1
        )));
    }

    #[test]
    fn an_integer_addition_lowers_to_the_inlined_fast_path() {
        let code = compile_foo("1 + 2");
        assert!(
            has(&code, |instruction| matches!(
                instruction,
                Instruction::IntegerBinary {
                    operation: IntegerOperation::Add,
                    ..
                }
            )),
            "{:#?}",
            code.instructions()
        );
    }

    #[test]
    fn the_inlined_fast_path_keeps_the_ordinary_send_as_its_fallback() {
        // The fallback is what raises every error the fast path declines to
        // handle, so it must still be emitted -- and it must be a real send,
        // not a second copy of the arithmetic.
        let code = compile_foo("1 + 2");
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::SendPublic { selector, .. } if selector.as_str() == "+"
        )));
    }

    /// The whole design rests on the fallback being the very send the
    /// ordinary lowering would have emitted, over the very registers the
    /// guard already read: lowering the message a second time would evaluate
    /// the argument's own side effects twice whenever the guard fails.
    #[test]
    fn the_inlined_fast_paths_fallback_reuses_the_registers_the_guard_read() {
        let code = compile_foo("x + (x bump)");
        let instructions = code.instructions();
        let Some(Instruction::IntegerBinary { left, right, .. }) = instructions
            .iter()
            .find(|instruction| matches!(instruction, Instruction::IntegerBinary { .. }))
        else {
            panic!("expected the fast path: {:#?}", code.instructions())
        };
        let instructions = code.instructions();
        let Some(Instruction::SendPublic {
            receiver_register,
            argument_sources,
            ..
        }) = instructions.iter().find(
            |instruction| matches!(instruction, Instruction::SendPublic { selector, .. } if selector.as_str() == "+"),
        ) else {
            panic!("expected the fallback send: {:#?}", code.instructions())
        };
        assert_eq!(*receiver_register, *left);
        assert_eq!(*argument_sources, vec![*right]);
        assert_eq!(
            code.instructions()
                .iter()
                .filter(|instruction| matches!(
                    instruction,
                    Instruction::SendPublic { selector, .. } if selector.as_str() == "bump"
                ))
                .count(),
            1,
            "the argument is evaluated once, on both paths: {:#?}",
            code.instructions()
        );
    }

    #[test]
    fn a_super_send_of_an_inlinable_selector_is_not_inlined() {
        // `super + x` starts its lookup above the receiver's class, which the
        // guard cannot reproduce, so it stays an ordinary send.
        let code = compile_foo("super + 2");
        assert!(!has(&code, |instruction| matches!(
            instruction,
            Instruction::IntegerBinary { .. }
        )));
    }

    /// The inlined form must ask the same question about its receiver's
    /// register that the ordinary send asks. `total` is assigned inside an
    /// inlinable conditional's block, so the fallback closure that
    /// conditional's guard failure builds writes `total` by reference, and
    /// any later send may be the one that runs it -- including the
    /// argument's own send and the fast path's fallback. The guard must
    /// therefore read a copy, not `total`'s own register. Getting this wrong
    /// is silent wrong output at exit status 0.
    #[test]
    fn a_local_read_across_an_inlined_arithmetic_fallback_is_copied() {
        let code = lower_whole_method(
            indoc! {"
                class X = Object new (
                ) (
                  bump: value = ( ^value + 1 )
                  foo = (
                    | total |
                    total:: 1.
                    true ifTrue: [total:: 2].
                    ^total + (self bump: total)
                  )
                ) : ()
            "},
            1,
        );
        // `foo` takes no parameters, so its only local `total` is register 0.
        let instructions = code.instructions();
        let Some(Instruction::IntegerBinary { left, .. }) = instructions
            .iter()
            .find(|instruction| matches!(instruction, Instruction::IntegerBinary { .. }))
        else {
            panic!("expected the fast path: {:#?}", code.instructions())
        };
        assert_ne!(*left, 0, "`total` can be written before the guard reads it");
        assert!(
            code.instructions().contains(&Instruction::Move {
                destination: *left,
                source: 0
            }),
            "the guard reads the copy of `total`: {:#?}",
            code.instructions()
        );
    }

    #[test]
    fn while_true_compiles_to_a_back_edge_jump_and_a_non_boolean_signal() {
        let code = compile_foo("[x] whileTrue: [x]");
        let back_edge =
            code.instructions().iter().enumerate().find_map(
                |(index, instruction)| match instruction {
                    Instruction::Jump { target } if (*target as usize) < index => Some(*target),
                    _ => None,
                },
            );
        assert!(
            back_edge.is_some(),
            "expected a backward Jump, got {:#?}",
            code.instructions()
        );
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::SignalNonBoolean { .. }
        )));
        assert!(!has(&code, |instruction| matches!(
            instruction,
            Instruction::SendPublic { selector, .. } if selector.as_str() == "whileTrue:"
        )));
        assert!(
            !has(&code, |instruction| matches!(
                instruction,
                Instruction::MakeClosure { .. }
            )),
            "an inlined loop materializes no closures"
        );
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::BranchIfFalse { .. }
        )));
        let code = compile_foo("[x] whileFalse: [x]");
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::BranchIfTrue { .. }
        )));
    }

    #[test]
    fn a_reference_inside_an_inlined_block_addresses_the_enclosing_frame_directly() {
        // `x` inside the inlined block is `OuterLocal { depth: 1, index: 0 }`
        // to the resolver; at run time it is the current frame's register 0.
        let code = compile_foo("x ifTrue: [x] ifFalse: [nil]");
        assert!(!has(&code, |instruction| matches!(
            instruction,
            Instruction::LoadOuterLocal { .. }
        )));
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::Move { source: 0, .. }
        )));
    }

    /// The same block literal, lowered twice: converted on the inlined
    /// common path, by reference inside the fallback closure the guard's
    /// failure path builds out of the very same source
    /// (`docs/design/execution.md`, "Fallback blocks").
    #[test]
    fn a_real_block_nested_in_an_inlined_block_is_converted_and_its_fallback_copy_is_not() {
        let code = compile_foo("x ifTrue: [ [ x ] value ] ifFalse: [nil]");
        let closures: Vec<(Rc<CompiledCode>, Vec<u16>)> = code
            .instructions()
            .iter()
            .filter_map(|instruction| match instruction {
                Instruction::MakeClosure { code, copied, .. } => {
                    Some((code.clone(), copied.clone()))
                }
                _ => None,
            })
            .collect();
        // The inlined true arm's own `[ x ]`, then the two closures the
        // fallback send is given.
        assert_eq!(closures.len(), 3);
        let (inlined_arm_block, copied) = &closures[0];
        assert_eq!(*copied, vec![0], "handed the method's own register for x");
        assert_eq!(
            inlined_arm_block.instruction(0),
            Instruction::ImplicitReturn { register: 0 }
        );
        assert_eq!(inlined_arm_block.outer_reach, 0);
        // Inside the fallback closure the nested `[ x ]` reaches two frames
        // out: through the fallback closure's own frame to the method's.
        let (fallback_true_block, fallback_copied) = &closures[1];
        assert!(fallback_copied.is_empty());
        let instructions = fallback_true_block.instructions();
        let Some(Instruction::MakeClosure {
            code: nested,
            copied: nested_copied,
            ..
        }) = instructions.first()
        else {
            panic!("the fallback closure re-creates the nested block")
        };
        assert!(nested_copied.is_empty());
        assert_eq!(
            nested.instruction(0),
            Instruction::LoadOuterLocal {
                destination: 0,
                depth: 2,
                index: 0
            }
        );
        assert_eq!(fallback_true_block.outer_reach, 1);
    }

    #[test]
    fn a_return_inside_an_inlined_block_follows_the_enclosing_code_kind() {
        let code = compile_foo("x ifTrue: [^1] ifFalse: [^2]");
        assert_eq!(
            code.instructions()
                .iter()
                .filter(|instruction| matches!(instruction, Instruction::Return { .. }))
                .count(),
            3,
            "{:#?}",
            code.instructions()
        );
        assert!(!has(&code, |instruction| matches!(
            instruction,
            Instruction::NonLocalReturn { .. }
        )));
        // Inside a block body the same `^` is a non-local return.
        let code = compile_foo("[ x ifTrue: [^1] ifFalse: [^2] ]");
        let instructions = code.instructions();
        let Some(Instruction::MakeClosure { code: block, .. }) = instructions.first() else {
            panic!()
        };
        assert!(block
            .instructions()
            .iter()
            .any(|instruction| matches!(instruction, Instruction::NonLocalReturn { .. })));
    }

    #[test]
    fn inlined_statements_do_not_clobber_live_registers_of_the_enclosing_statement() {
        // `x foo: (x bar) with: (x ifTrue: [x baz. 1] ifFalse: [2])`: the
        // first argument's register must survive the inlined `x baz` statement.
        let code = compile_foo("x foo: (x bar) with: (x ifTrue: [x baz. 1] ifFalse: [2])");
        let bar_destination = code
            .instructions()
            .iter()
            .find_map(|instruction| match instruction {
                Instruction::SendPublic {
                    selector,
                    destination,
                    ..
                } if selector.as_str() == "bar" => Some(*destination),
                _ => None,
            })
            .expect("a bar send");
        let baz_destination = code
            .instructions()
            .iter()
            .find_map(|instruction| match instruction {
                Instruction::SendPublic {
                    selector,
                    destination,
                    ..
                } if selector.as_str() == "baz" => Some(*destination),
                _ => None,
            })
            .expect("a baz send");
        assert_ne!(bar_destination, baz_destination);
        let instructions = code.instructions();
        let Some(Instruction::SendPublic {
            selector, argument_sources, ..
        }) = instructions.iter().rev().find(
            |instruction| matches!(instruction, Instruction::SendPublic { selector, .. } if selector.as_str() == "foo:with:"),
        )
        else {
            panic!("expected a foo:with: send")
        };
        assert_eq!(selector.as_str(), "foo:with:");
        assert_eq!(argument_sources[0], bar_destination);
    }

    /// The evaluated arm moves the block's value into the result as it is:
    /// no literal `true` or `false` is loaded to coerce it.
    #[test]
    fn and_or_answer_the_block_value_and_short_circuit() {
        for source in ["x and: [x]", "x or: [x]"] {
            let code = compile_foo(source);
            assert!(
                !has(&code, |instruction| matches!(
                    instruction,
                    Instruction::LoadLiteral {
                        literal: BytecodeLiteral::Boolean(_),
                        ..
                    }
                )),
                "{source}"
            );
        }
        let code = compile_foo("x and: [x]");
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::SendPublic { selector, .. } if selector.as_str() == "and:"
        )));
        let code = compile_foo("x or: [x]");
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::SendPublic { selector, .. } if selector.as_str() == "or:"
        )));
    }

    #[test]
    fn mutually_exclusive_branch_arms_reuse_registers_instead_of_stacking() {
        // Each arm needs several live registers on its own (a chain of two
        // binary sends holding both operands alive), but the arms never run
        // together -- register count must stay bounded by the costliest single
        // arm, not grow with the sum of all three arms' code size. Before the
        // `emit_guarded_branch` scratch-allocator reset, this body compiled to
        // 15 registers; the arms now share scratch space and it compiles to 7
        // (`self` r0, `x` r1, guard+destination r2-r3, and the costliest arm's
        // own 3 scratch registers r4-r6).
        let code = compile_foo("x ifTrue: [(x + 1) + (x + 2)] ifFalse: [(x + 3) + (x + 4)]");
        assert_eq!(code.register_count, 7, "{:#?}", code.instructions());
    }

    #[test]
    fn inlined_statements_are_inside_the_enclosing_statements_program_counter_range() {
        let code = compile_foo("x ifTrue: [x foo. x bar] ifFalse: [nil]");
        assert_eq!(
            code.statement_program_counter.len(),
            1,
            "the inlined statements are part of statement 0"
        );
        assert_eq!(
            code.statement_program_counter[0].1,
            0..code.instructions().len()
        );
    }

    /// `foo<pattern> = ( | t0 t1 ... | <statements> )`, with `count`
    /// generated temporaries, lowered whole.
    fn lower_with_temporaries(pattern: &str, count: usize, statements: &str) -> CompiledCode {
        let temporaries: String = (0..count).map(|index| format!("t{index} ")).collect();
        lower_whole_method(
            &formatdoc! {"
                class X = Object new (
                ) (
                  foo{pattern} = ( | {temporaries}| {statements} )
                ) : ()
            "},
            0,
        )
    }

    /// The body `compiler::body_too_large` builds for a body of `kind`
    /// taking `argument_count` arguments: its one register follows them.
    fn assert_raises_too_large(code: &CompiledCode, kind: CodeKind, argument_count: u16) {
        assert_eq!(code.kind, kind);
        assert_eq!(
            code.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: argument_count,
                    literal: BytecodeLiteral::BodyTooLarge,
                },
                Instruction::ImplicitReturn {
                    register: argument_count
                },
            ]
        );
        assert_eq!(code.argument_count, argument_count);
        assert_eq!(code.register_count, argument_count + 1);
        assert_eq!(code.copied_count, 0);
        assert_eq!(code.statement_program_counter, vec![(0, 0..2)]);
    }

    fn loads_body_too_large(code: &CompiledCode) -> bool {
        has(code, |instruction| {
            matches!(
                instruction,
                Instruction::LoadLiteral {
                    literal: BytecodeLiteral::BodyTooLarge,
                    ..
                }
            )
        })
    }

    /// `u16::MAX - 1` temporaries and the scratch register `self` is
    /// answered from fill the ceiling exactly; one temporary more needs a
    /// register past it, and more temporaries than registers cannot even be
    /// laid out. Neither panics: both compile to the raising body.
    #[test]
    fn a_body_past_the_register_ceiling_compiles_to_one_that_raises() {
        let at_the_ceiling = lower_with_temporaries("", usize::from(u16::MAX) - 1, "");
        assert_eq!(at_the_ceiling.register_count, u16::MAX);
        assert!(!loads_body_too_large(&at_the_ceiling));

        let one_scratch_past = lower_with_temporaries("", usize::from(u16::MAX), "");
        assert_raises_too_large(&one_scratch_past, CodeKind::Method, 0);

        let locals_past = lower_with_temporaries(": a with: b", usize::from(u16::MAX) + 1, "^a");
        assert_raises_too_large(&locals_past, CodeKind::Method, 2);
    }

    /// The scratch registers alone can pass the ceiling: a tuple's elements
    /// are each in a register of their own until `MakeArray` reads them.
    #[test]
    fn scratch_registers_past_the_ceiling_compile_to_a_body_that_raises() {
        let tuple = |count: usize| {
            let elements: String = (0..count).map(|_| "1. ").collect();
            compile_foo(&format!("{{{elements}}}"))
        };
        // `x` is r0, so `u16::MAX - 2` elements and the tuple's own register
        // fill it exactly.
        let at_the_ceiling = tuple(usize::from(u16::MAX) - 2);
        assert_eq!(at_the_ceiling.register_count, u16::MAX);
        assert!(!loads_body_too_large(&at_the_ceiling));
        assert_raises_too_large(&tuple(usize::from(u16::MAX) - 1), CodeKind::Method, 1);
    }

    /// A block is judged on its own: one whose copied value and temporaries
    /// pass the ceiling raises, its creator hands it no copied value, and
    /// the method that creates it compiles as it would around any block.
    #[test]
    fn a_block_past_the_register_ceiling_raises_and_its_creator_hands_it_nothing() {
        let temporaries: String = (0..usize::from(u16::MAX))
            .map(|index| format!("t{index} "))
            .collect();
        let code = compile_foo(&format!("[ | {temporaries}| x ]"));
        assert!(!loads_body_too_large(&code));
        let Some(Instruction::MakeClosure {
            code: block,
            copied,
            ..
        }) = code
            .instructions()
            .into_iter()
            .find(|instruction| matches!(instruction, Instruction::MakeClosure { .. }))
        else {
            panic!("the method creates the block")
        };
        assert!(copied.is_empty());
        assert_raises_too_large(&block, CodeKind::Block, 0);
    }

    /// More parameters than a `u16` counts: the raising block takes
    /// `u16::MAX` of them, all its registers, and loads into the last, which
    /// the literal raises before writing; its `MakeClosure` names as many,
    /// so the two agree.
    #[test]
    fn a_block_with_more_parameters_than_registers_takes_as_many_as_it_can_count() {
        let parameters: String = (0..=usize::from(u16::MAX))
            .map(|index| format!(":p{index} "))
            .collect();
        let code = compile_foo(&format!("[{parameters}| p0 ]"));
        let Some(Instruction::MakeClosure {
            code: block,
            parameters,
            copied,
            ..
        }) = code
            .instructions()
            .into_iter()
            .find(|instruction| matches!(instruction, Instruction::MakeClosure { .. }))
        else {
            panic!("the method creates the block")
        };
        assert_eq!(parameters.len(), usize::from(u16::MAX));
        assert!(copied.is_empty());
        assert_eq!(block.argument_count, u16::MAX);
        assert_eq!(block.register_count, u16::MAX);
        assert_eq!(
            block.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: u16::MAX - 1,
                    literal: BytecodeLiteral::BodyTooLarge,
                },
                Instruction::ImplicitReturn {
                    register: u16::MAX - 1
                },
            ]
        );
    }

    /// The methods the argument-slot tests send from: a keyword send to
    /// `self`, a send to a scratch receiver of a parameter, a super send of
    /// a send's value, and an implicit-receiver keyword send.
    const ARGUMENT_SLOT_SENDS: &str = indoc! {"
        class X = Object new (
        ) (
          foo: first bar: second = ( ^first )
          foo: first = ( ^first )
          a = ( ^self )
          b = ( ^self )
          toSelf = ( ^self foo: 1 bar: 2 )
          toScratch: x = ( ^self a foo: x )
          toSuper = ( ^super foo: (b bar) )
          implicit = ( ^foo: 3 )
        ) : ()
    "};

    /// The receiver register, the argument base and the argument sources of
    /// the one send of `selector` in `code`; no receiver for a lexical or a
    /// super send.
    fn argument_slots_of(code: &CompiledCode, selector: &str) -> (Option<u16>, u16, Vec<u16>) {
        code.instructions()
            .into_iter()
            .find_map(|instruction| match instruction {
                Instruction::SendPublic {
                    selector: sent,
                    receiver_register,
                    argument_base,
                    argument_sources,
                    ..
                }
                | Instruction::SendInlinable {
                    selector: sent,
                    receiver_register,
                    argument_base,
                    argument_sources,
                    ..
                } if sent.as_str() == selector => {
                    Some((Some(receiver_register), argument_base, argument_sources))
                }
                Instruction::SendLexical {
                    selector: sent,
                    argument_base,
                    argument_sources,
                    ..
                }
                | Instruction::SendSuper {
                    selector: sent,
                    argument_base,
                    argument_sources,
                    ..
                } if sent.as_str() == selector => Some((None, argument_base, argument_sources)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("expected a `{selector}` send: {:?}", code.instructions()))
    }

    /// A send of `selector` in `code` names `arity` contiguous slots, at or
    /// above `first_scratch` and above a scratch receiver, inside the body's
    /// registers; answers its base and its sources.
    fn assert_contiguous_slots(
        code: &CompiledCode,
        selector: &str,
        arity: usize,
        first_scratch: u16,
    ) -> (u16, Vec<u16>) {
        let (receiver, argument_base, argument_sources) = argument_slots_of(code, selector);
        assert_eq!(argument_sources.len(), arity, "{:?}", code.instructions());
        assert!(
            argument_base >= first_scratch,
            "the slots of `{selector}` start at r{argument_base}, below the first scratch register r{first_scratch}"
        );
        if let Some(receiver) = receiver.filter(|receiver| *receiver >= first_scratch) {
            assert!(
                argument_base > receiver,
                "the slots of `{selector}` start at r{argument_base}, not after its receiver r{receiver}"
            );
        }
        assert!(
            u32::from(argument_base) + arity as u32 <= u32::from(code.register_count),
            "the slots of `{selector}` run past the body's {} registers",
            code.register_count
        );
        (argument_base, argument_sources)
    }

    #[test]
    fn a_send_to_self_names_contiguous_argument_slots() {
        let code = lower_whole_method(ARGUMENT_SLOT_SENDS, 4);
        assert_contiguous_slots(&code, "foo:bar:", 2, 0);
    }

    /// A parameter is never copied: it is the argument's source, and the
    /// send itself copies it into its slot.
    #[test]
    fn a_send_to_a_scratch_receiver_names_its_slots_after_the_receiver() {
        let code = lower_whole_method(ARGUMENT_SLOT_SENDS, 5);
        let (_, sources) = assert_contiguous_slots(&code, "foo:", 1, 1);
        assert_eq!(sources, vec![0], "`x` is register 0, passed as it is");
    }

    #[test]
    fn a_super_send_names_contiguous_argument_slots() {
        let code = lower_whole_method(ARGUMENT_SLOT_SENDS, 6);
        assert_contiguous_slots(&code, "foo:", 1, 0);
    }

    #[test]
    fn an_implicit_receiver_send_names_contiguous_argument_slots() {
        let code = lower_whole_method(ARGUMENT_SLOT_SENDS, 7);
        assert_contiguous_slots(&code, "foo:", 1, 0);
    }

    /// The value `x` had when the first argument was evaluated, not 3: the
    /// protective copy goes into the first slot, which is then its source.
    #[test]
    fn a_local_a_later_argument_assigns_is_copied_into_its_slot() {
        let source = indoc! {"
            class X = Object new (
            ) (
              foo: first bar: second = ( ^first )
              m = ( | x ::= 1. | ^self foo: x bar: (x:: 3) )
            ) : ()
        "};
        let code = lower_whole_method(source, 1); // `x` is register 0
        let (argument_base, sources) = assert_contiguous_slots(&code, "foo:bar:", 2, 1);
        assert_ne!(
            sources[0], 0,
            "`x` is register 0, which the second argument writes"
        );
        assert_eq!(sources[0], argument_base, "the copy is the first slot");
        assert!(
            code.instructions().contains(&Instruction::Move {
                destination: argument_base,
                source: 0
            }),
            "the copy out of `x` is a Move into the slot: {:?}",
            code.instructions()
        );
    }

    /// The methods the tests of arguments lowered into their slots send
    /// from: a send's value and a literal, and an inlined conditional.
    const ARGUMENTS_INTO_SLOTS: &str = indoc! {"
        class X = Object new (
        ) (
          foo: first baz: second = ( ^first )
          foo: first = ( ^first )
          a = ( ^self )
          straight = ( ^self foo: (a bar) baz: 3 )
          branch: c = ( ^self foo: (c ifTrue: [1] ifFalse: [2]) )
        ) : ()
    "};

    /// A send's value and a literal are produced in their slots: both are
    /// their own sources, and nothing is moved.
    #[test]
    fn a_send_and_a_literal_argument_are_lowered_into_their_slots() {
        let code = lower_whole_method(ARGUMENTS_INTO_SLOTS, 3);
        let (argument_base, sources) = assert_contiguous_slots(&code, "foo:baz:", 2, 0);
        assert_eq!(
            sources,
            vec![argument_base, argument_base + 1],
            "{:?}",
            code.instructions()
        );
        assert!(
            !has(&code, |instruction| matches!(
                instruction,
                Instruction::Move { .. }
            )),
            "no argument is moved: {:?}",
            code.instructions()
        );
    }

    /// An inlined conditional's value is in the branch's result register,
    /// which both arms write, so the send copies it into its slot.
    #[test]
    fn an_inlined_conditional_argument_is_copied_by_the_send() {
        let code = lower_whole_method(ARGUMENTS_INTO_SLOTS, 4);
        let (argument_base, sources) = assert_contiguous_slots(&code, "foo:", 1, 1);
        assert_ne!(sources[0], argument_base, "{:?}", code.instructions());
        assert!(
            has(&code, |instruction| matches!(
                instruction,
                Instruction::Move { destination, .. } if *destination == sources[0]
            )),
            "the source is the branch's result register: {:?}",
            code.instructions()
        );
    }

    /// An inlined integer operation's fast path and its fallback send both
    /// write its destination, which is the slot: the send copies nothing.
    #[test]
    fn an_inlined_integer_operation_argument_is_lowered_into_its_slot() {
        let source = indoc! {"
            class X = Object new (
            ) (
              foo: first = ( ^first )
              m: n = ( ^self foo: n - 1 )
            ) : ()
        "};
        let code = lower_whole_method(source, 1);
        let (argument_base, sources) = assert_contiguous_slots(&code, "foo:", 1, 1);
        assert_eq!(sources, vec![argument_base], "{:?}", code.instructions());
        assert!(
            has(&code, |instruction| matches!(
                instruction,
                Instruction::IntegerBinary { destination, .. } if *destination == argument_base
            )),
            "the fast path writes the slot: {:?}",
            code.instructions()
        );
        assert!(
            !has(&code, |instruction| matches!(
                instruction,
                Instruction::Move { destination, .. } if *destination == argument_base
            )),
            "nothing is moved into the slot: {:?}",
            code.instructions()
        );
    }
}
