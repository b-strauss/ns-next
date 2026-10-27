//! The reference lowering: a second, deliberately naive
//! `Expression`/`Statement` -> `Instruction` lowering, which exists to be
//! compared against `lower.rs`. Nothing is inlined, every value gets a register
//! nothing else writes, and every read of a local is copied. See
//! `docs/design/compiler.md`, "The reference lowering" -- in particular why
//! this module shares so little with `lower.rs`: whatever the two share, the
//! comparison cannot check.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;

use ns_syntax::ast::{self, Expression, ExpressionKind, Statement, StatementKind};
use ns_syntax::Span;

use super::control_flow;
use super::lower::{lower_literal, selector_of};
use super::simultaneous;
use crate::bytecode::leftovers::{self, LeftoverRefusal};
use crate::bytecode::{
    register_written, registers_read, BodyLimit, CallSite, CodeBuilder, CodeKind, CompiledCode,
    Instruction, Literal as BytecodeLiteral, HEADER_REGISTERS,
};
use crate::mixin::Mixin;
use crate::types::Binding;
use crate::Symbol;

type Bindings = Rc<HashMap<Span, Binding>>;

pub(crate) fn compile_method(method: &ast::MethodDeclaration, bindings: &Bindings) -> CompiledCode {
    let mut body = Body::new(
        CodeKind::Method,
        super::argument_count_of(&method.pattern),
        method.body.temporaries.len(),
        bindings,
    );
    body.local_initializers(&method.body.temporaries);
    body.statements_and_end(&method.body.statements);
    body.finish(method.span)
}

pub(crate) fn compile_mixin_bodies(mixin: &mut Mixin, bindings: &Bindings) {
    let own = Rc::new(initializer(mixin, bindings, None));
    mixin.initializer_code = Some(match super::Prologue::of(mixin, true) {
        Some(prologue) => Rc::new(initializer(mixin, bindings, Some(prologue))),
        None => Rc::clone(&own),
    });
    mixin.standalone_initializer_code = if mixin.mixin_applications.is_empty() || !mixin.has_body {
        None
    } else {
        super::Prologue::of(mixin, false)
            .map(|prologue| Rc::new(initializer(mixin, bindings, Some(prologue))))
    };
    mixin.own_initializer_code = Some(own);
    mixin.superclass_clause_code = superclass_clause(mixin, bindings).map(Rc::new);
}

/// `prologue`'s clauses when there is one -- the full or the standalone
/// body -- then each own slot's initializer followed by `InitializeSlot`,
/// one statement apiece, then the `initExprs`, then `self`.
/// `InitializeSlot`'s index is the slot's position among the mixin's own
/// slots, never an absolute field index (`docs/design/compiler.md`, "One
/// body per mixin"). A simultaneous slot (§6.3.2) stores its initializer's
/// future provisionally instead, and the clause's end reads each such slot
/// back and stores it resolved, in declaration order.
fn initializer(
    mixin: &Mixin,
    bindings: &Bindings,
    prologue: Option<super::Prologue<'_>>,
) -> CompiledCode {
    let mut body = Body::new(
        CodeKind::Initializer,
        super::factory_argument_count(mixin),
        0,
        bindings,
    );
    let mut source_span: Option<Span> = None;
    if let Some(prologue) = prologue {
        body.prologue(prologue.superclass, prologue.applied, prologue.has_body);
        source_span = prologue.merge_spans(source_span);
    }
    for (own_index, slot) in mixin.slots.iter().enumerate() {
        // A lazy slot's initializer is its getter's (`compile_lazy_slot_getter`).
        if slot.lazy {
            continue;
        }
        let Some(declaration) = &slot.declaration else {
            continue;
        };
        let Some(initializer) = &declaration.initializer else {
            continue;
        };
        let start = body.code.len();
        match simultaneous::future_block(declaration) {
            Some(block) => {
                let closure = body.value(&block);
                let future = body.fresh();
                body.emit(Instruction::MakeFuture {
                    destination: future,
                    closure,
                });
                body.emit(Instruction::InitializeSlotProvisionally {
                    source: future,
                    own_index: super::own_slot_index(own_index),
                });
            }
            None => {
                let source = body.value(initializer);
                body.emit(Instruction::InitializeSlot {
                    source,
                    own_index: super::own_slot_index(own_index),
                });
            }
        }
        body.record_statement(start);
        source_span = Some(super::merge_span(source_span, declaration.span));
    }
    for (own_index, slot) in mixin.slots.iter().enumerate() {
        let simultaneous_with_initializer = slot.declaration.as_ref().is_some_and(|declaration| {
            declaration.simultaneous && declaration.initializer.is_some()
        });
        if slot.lazy || !simultaneous_with_initializer {
            continue;
        }
        let start = body.code.len();
        let slot_value = body.fresh();
        body.emit(Instruction::LoadOwnSlot {
            destination: slot_value,
            own_index: super::own_slot_index(own_index),
        });
        let resolved = body.fresh();
        body.emit(Instruction::ResolveFuture {
            destination: resolved,
            source: slot_value,
        });
        body.emit(Instruction::InitializeSlot {
            source: resolved,
            own_index: super::own_slot_index(own_index),
        });
        body.record_statement(start);
    }
    for statement in &mixin.initializer_expressions {
        source_span = Some(super::merge_span(
            source_span,
            super::statement_span(statement),
        ));
    }
    body.statements_and_end(&mixin.initializer_expressions);
    body.finish(source_span.unwrap_or(Span::new(0, 0)))
}

/// A lazy slot's getter (§6.3.3): the filled slot answered by
/// `ReturnSlotIfFilled`, as the optimized lowering answers it; otherwise
/// `initializer`'s value, stored with `InitializeSlot` as `initializer`'s
/// per-slot step stores it, then returned. A zero-argument method body with
/// no temporaries.
pub(crate) fn compile_lazy_slot_getter(
    initializer: &Expression,
    own_index: u32,
    span: Span,
    bindings: &Bindings,
) -> CompiledCode {
    let mut body = Body::new(CodeKind::Method, 0, 0, bindings);
    body.emit(Instruction::ReturnSlotIfFilled { own_index });
    let start = body.code.len();
    let source = body.value(initializer);
    body.emit(Instruction::InitializeSlot { source, own_index });
    // One statement, its `Return` included, as a `^e` statement's is.
    body.emit(Instruction::Return { register: source });
    body.record_statement(start);
    body.finish(span)
}

/// The superclass clause as a single `^ec`. The expression is
/// `interpreter::nested`'s, the one definition of what a clause evaluates.
fn superclass_clause(mixin: &Mixin, bindings: &Bindings) -> Option<CompiledCode> {
    let declaration = mixin.declaration.as_ref()?;
    let clause = declaration.superclass.as_ref()?;
    let statements = [Statement::of_return(
        crate::interpreter::nested::superclass_clause_expression(
            clause,
            &declaration.mixin_applications,
        ),
    )];
    let mut body = Body::new(CodeKind::SuperclassClause, 0, 0, bindings);
    body.statements_and_end(&statements);
    Some(body.finish(clause.span))
}

/// One body being lowered: a method, a block, an initializer or a
/// superclass-clause body.
struct Body<'a> {
    bindings: &'a Bindings,
    kind: CodeKind,
    /// Saturated at `u16::MAX`: only a body that is `too_large` meets a
    /// count past it.
    argument_count: u16,
    /// The first register above the named locals, which follow the
    /// parameters past the header's numbers (`HEADER_REGISTERS`). Every
    /// register from here on is written by exactly one instruction.
    first_scratch: u16,
    next_register: u16,
    /// The argument count of every body this one is nested in, outermost
    /// first: what maps an outer local's resolver index onto the register
    /// it sits in, in the frame that declares it.
    enclosing_argument_counts: Vec<u16>,
    /// The limit the body passed, if any: more registers than a `u16`
    /// numbers, more than `ARGUMENT_LIMIT` arguments taken or sent, more
    /// than `INSTRUCTION_LIMIT` instructions (`docs/design/compiler.md`,
    /// "Limits"). Set the moment it is known, the first one met kept, and
    /// from then on nothing is emitted: `finish` answers
    /// `super::body_too_large` instead, never a body whose register indices
    /// wrapped or that grew past a limit.
    too_large: Option<BodyLimit>,
    code: CodeBuilder,
    statement_program_counter: Vec<(usize, Range<usize>)>,
}

impl<'a> Body<'a> {
    fn new(
        kind: CodeKind,
        argument_count: usize,
        temporary_count: usize,
        bindings: &'a Bindings,
    ) -> Self {
        // Checked in every build: a wrapped sum would place scratch
        // registers on top of the named locals. A body whose named locals do
        // not fit is too large before it lowers anything, and lowers
        // nothing (`local_initializers`, `statements_and_end`): the
        // resolver's indices for its locals past `u16::MAX` name nothing.
        let first_scratch = super::named_locals_fit(argument_count, temporary_count)
            .then(|| {
                u16::try_from(argument_count + usize::from(HEADER_REGISTERS) + temporary_count).ok()
            })
            .flatten();
        let too_large = match first_scratch {
            None => Some(BodyLimit::Registers),
            Some(_) => super::parameter_limit_passed(argument_count),
        };
        let first_scratch = first_scratch.unwrap_or(u16::MAX);
        Body {
            bindings,
            kind,
            argument_count: super::saturated_count(argument_count),
            first_scratch,
            next_register: first_scratch,
            enclosing_argument_counts: Vec::new(),
            too_large,
            code: CodeBuilder::new(),
            statement_program_counter: Vec::new(),
        }
    }

    /// The register the local at resolver index `index` of the body
    /// `depth` levels out -- this one at 0 -- sits in: a parameter's is its
    /// index, a temporary's is past the header's numbers.
    fn local_register(&self, depth: u16, index: u16) -> u16 {
        let argument_count = match usize::from(depth) {
            0 => self.argument_count,
            depth => self.enclosing_argument_counts[self.enclosing_argument_counts.len() - depth],
        };
        if index < argument_count {
            index
        } else {
            index.checked_add(HEADER_REGISTERS).expect(
                "internal: a local's register past u16::MAX in a body whose locals were checked",
            )
        }
    }

    /// A register nothing has written yet. Checked in every build rather than
    /// only under `debug_assertions`: a register index that wrapped would be
    /// silently wrong code. Past the ceiling it marks the body too large and
    /// answers the last register again, which `finish` never keeps.
    fn fresh(&mut self) -> u16 {
        let register = self.next_register;
        match register.checked_add(1) {
            Some(next) => self.next_register = next,
            None => self.pass_a_limit(BodyLimit::Registers),
        }
        register
    }

    /// Records that the body passed `limit`, unless it passed one before.
    fn pass_a_limit(&mut self, limit: BodyLimit) {
        self.too_large.get_or_insert(limit);
    }

    /// Emits `instruction`, unless the body is too large already or the
    /// instruction would make it so (`super::emitting_passes_a_limit`).
    fn emit(&mut self, instruction: Instruction) {
        if self.too_large.is_some() {
            return;
        }
        match super::emitting_passes_a_limit(self.code.len(), &instruction) {
            Some(limit) => self.pass_a_limit(limit),
            None => {
                self.code.emit(instruction);
            }
        }
    }

    fn binding(&self, span: Span) -> Option<Binding> {
        self.bindings.get(&span).copied()
    }

    fn record_statement(&mut self, start: usize) {
        let index = self.statement_program_counter.len();
        self.statement_program_counter
            .push((index, start..self.code.len()));
    }

    /// An initializer's clauses, in `lower.rs`'s order
    /// (`Lowerer::lower_instance_initialization_prologue`): each `<:`
    /// clause's check, the topmost first, as one statement; the superclass
    /// clause's arguments and its initialization; each `<:` clause's
    /// arguments and its level's initialization, from the lowest level up.
    /// The declaration's own level is the one the body runs at, so `<:`
    /// clause `k` (from 1) of `n` names the level `n - k + 1` steps up and
    /// the superclass clause the class `n + 1` steps up -- one step fewer
    /// each for a body-less declaration (`has_body` false), whose body runs
    /// at the level its last clause adds (`super::superclass_steps`).
    fn prologue(
        &mut self,
        superclass: Option<&ast::SuperclassClause>,
        applied: &[ast::SuperclassClause],
        has_body: bool,
    ) {
        let count = applied.len();
        let own_level = usize::from(has_body);
        let superclass_steps = super::superclass_steps(count, own_level);
        // The level clause `index` (from 0) adds: below the superclass
        // clause's class, whose steps fit a `u16`.
        let steps_of = |index: usize| {
            u16::try_from(count + own_level - 1 - index)
                .expect("internal: more `<:` clauses than CLAUSE_INDEX_LIMIT reached the compiler")
        };
        if count > 0 {
            let start = self.code.len();
            for (index, clause) in applied.iter().enumerate().rev() {
                self.emit(Instruction::CheckAppliedClause {
                    steps: steps_of(index),
                    selector: super::clause_selector(Some(clause)),
                });
            }
            self.record_statement(start);
        }
        let start = self.code.len();
        let (argument_base, argument_sources) = self.arguments(super::clause_arguments(superclass));
        let destination = self.fresh();
        self.emit(Instruction::InitializeSuperclass {
            steps: superclass_steps,
            selector: super::clause_selector(superclass),
            argument_base,
            argument_sources,
            destination,
        });
        self.record_statement(start);
        for (index, clause) in applied.iter().enumerate() {
            let start = self.code.len();
            let (argument_base, argument_sources) =
                self.arguments(super::clause_arguments(Some(clause)));
            let destination = self.fresh();
            self.emit(Instruction::InitializeAppliedLevel {
                steps: steps_of(index),
                argument_base,
                argument_sources,
                destination,
            });
            self.record_statement(start);
        }
    }

    /// Each local slot with an initializer, in declaration order, as one
    /// statement: its expression, then a `StoreLocal` into the slot's own
    /// register, which follows the parameters (§3.6) -- the same shape
    /// `lower.rs`'s `lower_local_initializers` emits, in this module's own
    /// idiom. A simultaneous temporary (§6.3.2) stores its initializer's
    /// future instead, and the clause's end reads each one back and stores
    /// it resolved, in declaration order.
    fn local_initializers(&mut self, temporaries: &[ast::SlotDeclaration]) {
        if self.too_large.is_some() {
            return;
        }
        for (position, temporary) in temporaries.iter().enumerate() {
            if self.too_large.is_some() {
                // Nothing more is kept (`finish`): stop lowering.
                return;
            }
            let Some(initializer) = &temporary.initializer else {
                continue;
            };
            let start = self.code.len();
            let source = match simultaneous::future_block(temporary) {
                Some(block) => {
                    let closure = self.value(&block);
                    let future = self.fresh();
                    self.emit(Instruction::MakeFuture {
                        destination: future,
                        closure,
                    });
                    future
                }
                None => self.value(initializer),
            };
            let slot =
                self.local_register(0, super::temporary_index(self.argument_count, position));
            self.emit(Instruction::StoreLocal { source, slot });
            self.record_statement(start);
        }
        for (position, temporary) in temporaries.iter().enumerate() {
            if self.too_large.is_some() {
                // Nothing more is kept (`finish`): stop lowering.
                return;
            }
            if !temporary.simultaneous || temporary.initializer.is_none() {
                continue;
            }
            let start = self.code.len();
            let slot =
                self.local_register(0, super::temporary_index(self.argument_count, position));
            let copy = self.fresh();
            self.emit(Instruction::Move {
                destination: copy,
                source: slot,
            });
            let resolved = self.fresh();
            self.emit(Instruction::ResolveFuture {
                destination: resolved,
                source: copy,
            });
            self.emit(Instruction::StoreLocal {
                source: resolved,
                slot,
            });
            self.record_statement(start);
        }
    }

    fn statements_and_end(&mut self, statements: &[Statement]) {
        if self.too_large.is_some() {
            return;
        }
        let mut last_value = None;
        for statement in statements {
            if self.too_large.is_some() {
                // Nothing more is kept (`finish`): stop lowering.
                return;
            }
            last_value = self.statement(statement);
        }
        self.end(
            matches!(
                statements.last().map(|statement| &statement.kind),
                Some(StatementKind::Return(_))
            ),
            last_value,
        );
    }

    /// One of the body's own statements, recorded in
    /// `statement_program_counter`. Answers the statement's value register, or
    /// `None` when it emitted a return.
    fn statement(&mut self, statement: &Statement) -> Option<u16> {
        let start = self.code.len();
        let value = match &statement.kind {
            StatementKind::Expression(expression) => Some(self.value(expression)),
            StatementKind::Return(expression) => {
                let register = self.value(expression);
                match self.kind {
                    CodeKind::Method | CodeKind::SuperclassClause => {
                        self.emit(Instruction::Return { register });
                        None
                    }
                    CodeKind::Block => {
                        self.emit(Instruction::NonLocalReturn { register });
                        None
                    }
                    // A top-level `^` in an initializer has no method to
                    // return from: it is evaluated and its value discarded.
                    CodeKind::Initializer => Some(register),
                }
            }
        };
        self.record_statement(start);
        value
    }

    /// What ends a body whose last statement did not: a method answers
    /// `self`, an initializer always does (its `^` never returns), and a block
    /// or a superclass-clause body answers its last statement's value, `nil`
    /// when it has none.
    fn end(&mut self, ends_in_return: bool, last_value: Option<u16>) {
        match self.kind {
            CodeKind::Initializer => self.return_self(),
            CodeKind::Method if !ends_in_return => self.return_self(),
            CodeKind::Block | CodeKind::SuperclassClause if !ends_in_return => {
                let register = match last_value {
                    Some(register) => register,
                    None => self.literal(BytecodeLiteral::Nil, None),
                };
                self.emit(Instruction::ImplicitReturn { register });
            }
            CodeKind::Method | CodeKind::Block | CodeKind::SuperclassClause => {}
        }
    }

    fn return_self(&mut self) {
        let register = self.fresh();
        self.emit(Instruction::LoadSelf {
            destination: register,
        });
        self.emit(Instruction::ImplicitReturn { register });
    }

    fn literal(&mut self, literal: BytecodeLiteral, into: Option<u16>) -> u16 {
        let destination = self.target_register(into);
        self.emit(Instruction::LoadLiteral {
            destination,
            literal,
        });
        destination
    }

    /// Lowers `expression` and answers the register holding its value -- always
    /// a register this lowering wrote for it and will never write again.
    fn value(&mut self, expression: &Expression) -> u16 {
        self.value_to(expression, None)
    }

    /// `value`, the value produced in `into` when it is given -- a send's
    /// argument slot (`arguments`) -- and otherwise in a fresh register.
    /// Every expression's value is written by one instruction of its own,
    /// so it can be produced in any register: answers `into` when given.
    fn value_to(&mut self, expression: &Expression, into: Option<u16>) -> u16 {
        match &expression.kind {
            ExpressionKind::Literal(literal) => self.literal(lower_literal(literal), into),
            ExpressionKind::Identifier(id) => self.read(id, into),
            ExpressionKind::Assign { target, value, .. } => self.assign(target, value, into),
            ExpressionKind::Send(send) => self.send(send, into),
            ExpressionKind::Cascade(cascade) => self.cascade(cascade, into),
            ExpressionKind::Block(block) => self.block(block, into),
            ExpressionKind::DynamicArray { elements, .. } => {
                let element_registers = self.values(elements);
                let destination = self.target_register(into);
                self.emit(Instruction::MakeArray {
                    destination,
                    element_registers,
                });
                destination
            }
        }
    }

    /// Left to right, each into a register of its own.
    fn values(&mut self, expressions: &[Expression]) -> Vec<u16> {
        expressions
            .iter()
            .map(|expression| self.value(expression))
            .collect()
    }

    /// A send's arguments: one slot per argument, allocated at once after
    /// everything else the send reads, then each argument, left to right,
    /// produced in its slot (`value_to`) -- a named local by the `Move`
    /// that is its read -- so every source is its slot, and the optimized
    /// lowering's copy inside the send is what the two lowerings are
    /// compared on (`docs/design/compiler.md`, "The reference lowering").
    /// Answers the first slot and the sources.
    fn arguments(&mut self, arguments: &[Expression]) -> (u16, Vec<u16>) {
        let argument_base = self.next_register;
        let slots: Vec<u16> = arguments.iter().map(|_| self.fresh()).collect();
        for (slot, argument) in slots.iter().zip(arguments) {
            let value = self.value_to(argument, Some(*slot));
            debug_assert!(
                self.too_large.is_some() || value == *slot,
                "internal: the reference lowering produced an argument in r{value}, not its slot r{slot}"
            );
        }
        (argument_base, slots)
    }

    /// The register an instruction that produces a value writes: `into`
    /// when given, or else a fresh one.
    fn target_register(&mut self, into: Option<u16>) -> u16 {
        match into {
            Some(register) => register,
            None => self.fresh(),
        }
    }

    fn read(&mut self, id: &ast::Identifier, into: Option<u16>) -> u16 {
        if id.name == "self" {
            let destination = self.target_register(into);
            self.emit(Instruction::LoadSelf { destination });
            return destination;
        }
        match self.binding(id.span) {
            Some(Binding::Local(index)) => {
                let source = self.local_register(0, index);
                let destination = self.target_register(into);
                self.emit(Instruction::Move {
                    destination,
                    source,
                });
                destination
            }
            Some(Binding::OuterLocal { depth, index }) => {
                let index = self.local_register(depth, index);
                let destination = self.target_register(into);
                self.emit(Instruction::LoadOuterLocal {
                    destination,
                    depth,
                    index,
                });
                destination
            }
            Some(Binding::LexicalSend { depth }) => {
                let selector = Symbol::intern(&id.name);
                let (argument_base, argument_sources) = self.arguments(&[]);
                self.emit_send(into, |destination, call_site| Instruction::SendLexical {
                    depth,
                    selector,
                    argument_base,
                    argument_sources,
                    destination,
                    call_site,
                })
            }
            // A `super`-prefixed inheritance-clause receiver, `super Foo`:
            // §6.1's super send of `Foo` from the class declaring the clause.
            Some(Binding::SuperSend) => {
                let selector = Symbol::intern(&id.name);
                let (argument_base, argument_sources) = self.arguments(&[]);
                self.emit_send(into, |destination, call_site| Instruction::SendSuper {
                    selector,
                    argument_base,
                    argument_sources,
                    destination,
                    call_site,
                })
            }
            // No binding: the deferred unresolved-identifier error
            // (`docs/design/compiler.md`, "Compilation cannot fail") -- bare
            // `super`, and a module root's clause receivers, which the
            // resolver leaves unbound for the root rule and which never run.
            // `MirrorSend` and `EnclosingObject` mark a send, never an
            // identifier.
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

    /// The assignment's value is the register its value was computed into,
    /// never the local it was stored in. A setter's one argument slot is
    /// that register: nothing above it is live when the setter is sent,
    /// since every register this lowering allocates after it was written
    /// for the value and read already, or is an argument slot of an
    /// enclosing send not yet written.
    fn assign(&mut self, target: &ast::Identifier, value: &Expression, into: Option<u16>) -> u16 {
        let value_register = self.value_to(value, into);
        match self.binding(target.span) {
            Some(Binding::Local(index)) => {
                let slot = self.local_register(0, index);
                self.emit(Instruction::StoreLocal { source: value_register, slot });
            }
            Some(Binding::OuterLocal { depth, index }) => {
                let index = self.local_register(depth, index);
                self.emit(Instruction::StoreOuterLocal { source: value_register, depth, index });
            }
            Some(Binding::LexicalSend { depth }) => {
                let selector = Symbol::intern(&format!("{}:", target.name));
                self.emit_send(None, |destination, call_site| Instruction::SendLexical {
                    depth,
                    selector,
                    argument_base: value_register,
                    argument_sources: vec![value_register],
                    destination,
                    call_site,
                });
            }
            Some(Binding::SuperSend | Binding::MirrorSend | Binding::EnclosingObject { .. })
            | None => panic!(
                "internal: unresolved assignment target `{}` (the resolver should have rejected this)",
                target.name
            ),
        }
        value_register
    }

    fn send(&mut self, send: &ast::Send, into: Option<u16>) -> u16 {
        let selector = selector_of(&send.message);
        match self.binding(send.span) {
            Some(Binding::EnclosingObject { depth }) => {
                let destination = self.target_register(into);
                self.emit(Instruction::LoadEnclosingObject { destination, depth });
                destination
            }
            Some(Binding::SuperSend) => {
                let (argument_base, argument_sources) = self.arguments(&send.message.arguments);
                self.emit_send(into, |destination, call_site| Instruction::SendSuper {
                    selector,
                    argument_base,
                    argument_sources,
                    destination,
                    call_site,
                })
            }
            Some(Binding::LexicalSend { depth }) => {
                let (argument_base, argument_sources) = self.arguments(&send.message.arguments);
                self.emit_send(into, |destination, call_site| Instruction::SendLexical {
                    depth,
                    selector,
                    argument_base,
                    argument_sources,
                    destination,
                    call_site,
                })
            }
            // An ordinary send. The ones the optimized lowering inlines are
            // marked `SendInlinable`, which only the depth count reads
            // (`docs/design/compiler.md`, "How the limit counts"). A send to
            // the kernel's `vmMirror` is an ordinary send here, its failure
            // block a real closure; one whose block the optimized lowering
            // lowers inline is marked too, so that the block's activation is
            // exempt under the same condition as `ifTrue:`'s.
            Some(Binding::Local(_) | Binding::OuterLocal { .. } | Binding::MirrorSend) | None => {
                match control_flow::recognize(send) {
                    Some(control_flow::InlineForm::WhileTrue { condition, body }) => {
                        return self.while_loop(condition, body, true, into);
                    }
                    Some(control_flow::InlineForm::WhileFalse { condition, body }) => {
                        return self.while_loop(condition, body, false, into);
                    }
                    _ => {}
                }
                let receiver_register = self.value(&send.receiver);
                if control_flow::recognize(send).is_some()
                    || control_flow::recognize_mirror_send(self.bindings, send).is_some()
                {
                    self.inlinable_send(receiver_register, &send.message, into)
                } else {
                    self.public_send(receiver_register, &send.message, into)
                }
            }
        }
    }

    fn public_send(
        &mut self,
        receiver_register: u16,
        message: &ast::MessageSend,
        into: Option<u16>,
    ) -> u16 {
        let selector = selector_of(message);
        let (argument_base, argument_sources) = self.arguments(&message.arguments);
        self.emit_send(into, |destination, call_site| Instruction::SendPublic {
            selector,
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            call_site,
        })
    }

    fn inlinable_send(
        &mut self,
        receiver_register: u16,
        message: &ast::MessageSend,
        into: Option<u16>,
    ) -> u16 {
        let selector = selector_of(message);
        let (argument_base, argument_sources) = self.arguments(&message.arguments);
        self.emit_send(into, |destination, call_site| Instruction::SendInlinable {
            selector,
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            call_site,
        })
    }

    /// `[condition] whileTrue: [body]` (`while_true`) or `whileFalse:`,
    /// both literal blocks, as the optimized lowering inlines it: a loop,
    /// since a loop has to bottom out somewhere and `Closure>>whileTrue:`'s
    /// own body is this same shape. It stays naive: both blocks are real
    /// closures, made once, and each turn sends `value` to them, marked
    /// `SendInlinable` so that their activations are exempt from the depth
    /// count as the inlined bodies have none (`docs/design/compiler.md`,
    /// "The reference lowering", "How the limit counts"). A non-Boolean
    /// condition signals as the inlined guard does; the loop answers nil.
    fn while_loop(
        &mut self,
        condition: &ast::BlockExpression,
        body: &ast::BlockExpression,
        while_true: bool,
        into: Option<u16>,
    ) -> u16 {
        let condition_register = self.block(condition, None);
        let body_register = self.block(body, None);
        let loop_start = self.code.len();
        let condition_value = self.value_send(condition_register);
        let not_boolean_jump = self.code.len();
        self.emit(Instruction::JumpIfNotBoolean {
            register: condition_value,
            target: 0,
        });
        let exit_branch = self.code.len();
        self.emit(if while_true {
            Instruction::BranchIfFalse {
                register: condition_value,
                target: 0,
            }
        } else {
            Instruction::BranchIfTrue {
                register: condition_value,
                target: 0,
            }
        });
        self.value_send(body_register);
        self.emit(Instruction::Jump {
            target: Self::target(loop_start),
        });
        self.patch_target(not_boolean_jump, self.code.len());
        self.emit(Instruction::SignalNonBoolean {
            register: condition_value,
        });
        self.patch_target(exit_branch, self.code.len());
        self.literal(BytecodeLiteral::Nil, into)
    }

    /// `value` sent to the closure in `receiver_register`, marked as the
    /// activation of a block the optimized lowering inlines.
    fn value_send(&mut self, receiver_register: u16) -> u16 {
        let argument_base = self.next_register;
        self.emit_send(None, |destination, call_site| Instruction::SendInlinable {
            selector: Symbol::VALUE,
            receiver_register,
            argument_base,
            argument_sources: Vec::new(),
            destination,
            call_site,
        })
    }

    /// An instruction index as a jump target. At most `INSTRUCTION_LIMIT`,
    /// since `emit` emits nothing past it.
    fn target(index: usize) -> u32 {
        u32::try_from(index).expect("internal: a body holds at most INSTRUCTION_LIMIT instructions")
    }

    /// Points the branch at `instruction_index` at `target`. A body too
    /// large emits nothing more, so a branch it did not emit is not
    /// patched; its code is never kept.
    fn patch_target(&mut self, instruction_index: usize, target: usize) {
        if instruction_index < self.code.len() {
            self.code
                .patch_target(instruction_index, Self::target(target));
        }
    }

    /// The cascade's value is its last send's, produced in `into`.
    fn cascade(&mut self, cascade: &ast::Cascade, into: Option<u16>) -> u16 {
        let receiver_register = self.value(&cascade.receiver);
        let messages: Vec<&ast::MessageSend> = std::iter::once(&cascade.first)
            .chain(cascade.rest.iter())
            .collect();
        let (last, earlier) = messages
            .split_last()
            .expect("internal: a cascade has a first message");
        for message in earlier {
            self.public_send(receiver_register, message, None);
        }
        self.public_send(receiver_register, last, into)
    }

    fn block(&mut self, block: &ast::BlockExpression, into: Option<u16>) -> u16 {
        let parameters: Vec<Symbol> = block
            .parameters
            .iter()
            .map(|parameter| Symbol::intern(&parameter.name.name))
            .collect();
        let mut inner = Body::new(
            CodeKind::Block,
            parameters.len(),
            block.body.temporaries.len(),
            self.bindings,
        );
        inner.enclosing_argument_counts = self.enclosing_argument_counts.clone();
        inner.enclosing_argument_counts.push(self.argument_count);
        inner.local_initializers(&block.body.temporaries);
        inner.statements_and_end(&block.body.statements);
        let code = inner.finish(block.span);
        let parameters = super::parameters_of(parameters, &code);
        let destination = self.target_register(into);
        self.emit(Instruction::MakeClosure {
            destination,
            parameters: Rc::new(parameters),
            code: Rc::new(code),
            // The reference lowering is the by-reference oracle: it copies
            // nothing into a closure, ever.
            copied: Vec::new(),
        });
        destination
    }

    /// The reference lowering is the uncached oracle (`docs/design/compiler.md`,
    /// "The reference lowering"): every send it emits carries `UNCACHED`, so a
    /// send it compiles always runs the full lookup, never a cache hit.
    fn emit_send(
        &mut self,
        into: Option<u16>,
        build: impl FnOnce(u16, CallSite) -> Instruction,
    ) -> u16 {
        let destination = self.target_register(into);
        self.emit(build(destination, CallSite::UNCACHED));
        destination
    }

    fn finish(self, source: Span) -> CompiledCode {
        if let Some(limit) = self.too_large {
            return super::body_too_large(self.kind, self.argument_count, source, limit);
        }
        if cfg!(debug_assertions) {
            let instructions = self.code.instructions();
            check_shape(&instructions, self.first_scratch);
            check_leftovers(&instructions, self.first_scratch, self.next_register);
        }
        CompiledCode::from_builder(
            self.kind,
            self.code,
            self.argument_count,
            self.next_register,
            0,
            self.first_scratch,
            self.statement_program_counter,
            source,
        )
    }
}

/// The shape the reference lowering promises (`docs/design/compiler.md`,
/// "What it emits"), checked on every body in a debug build, which is what
/// the test suite runs: no branch but a `whileTrue:` or `whileFalse:`
/// loop's (`Body::while_loop`), its back edge the one jump backwards and
/// its two exits the only forward ones; every
/// register above the named locals written by exactly one instruction; a
/// named local written only by `StoreLocal`; and a named local read only by `Move`, every other operand being a
/// scratch register. The last is rule 3, "every read of a local is copied",
/// which is what keeps a value from changing under its consumer -- the
/// property the optimized lowering has to earn with copies of its own
/// ("The value-register invariant"). A send's argument slots are each its
/// own source, so the send copies nothing: every argument is produced in
/// its slot, a named local by the `Move` that is its read.
fn check_shape(instructions: &[Instruction], first_scratch: u16) {
    let mut written = HashSet::new();
    for (index, instruction) in instructions.iter().enumerate() {
        match instruction {
            Instruction::Jump { target } => assert!(
                (*target as usize) < index,
                "internal: the reference lowering jumped forwards: {instruction:?}"
            ),
            Instruction::JumpIfNotBoolean { target, .. }
            | Instruction::BranchIfFalse { target, .. }
            | Instruction::BranchIfTrue { target, .. } => assert!(
                (*target as usize) > index,
                "internal: the reference lowering branched backwards: {instruction:?}"
            ),
            _ => {}
        }
        if let Some((argument_base, argument_sources)) = instruction.argument_slots() {
            for (offset, source) in argument_sources.iter().enumerate() {
                let slot = usize::from(argument_base) + offset;
                assert!(
                    usize::from(*source) == slot,
                    "internal: the reference lowering left argument slot r{slot} to the send to copy: {instruction:?}"
                );
            }
        }
        assert!(
            !matches!(instruction, Instruction::IntegerBinary { .. }),
            "internal: the reference lowering emitted a branch: {instruction:?}"
        );
        // A `StoreLocal` writes a named local, the one way this lowering
        // may, and is checked below.
        let written_by_destination = match instruction {
            Instruction::StoreLocal { .. } => None,
            other => register_written(other),
        };
        if let Some(written_register) = written_by_destination {
            assert!(
                written_register >= first_scratch,
                "internal: the reference lowering wrote named local r{written_register} other than by StoreLocal"
            );
            assert!(
                written.insert(written_register),
                "internal: the reference lowering wrote r{written_register} twice"
            );
        }
        if let Instruction::StoreLocal { slot, .. } = instruction {
            assert!(
                *slot < first_scratch,
                "internal: the reference lowering stored into scratch register r{slot}: {instruction:?}"
            );
        }
        for operand in registers_read(instruction) {
            match instruction {
                Instruction::Move { .. } => assert!(
                    operand < first_scratch,
                    "internal: the reference lowering copied scratch register r{operand}: {instruction:?}"
                ),
                _ => assert!(
                    operand >= first_scratch,
                    "internal: the reference lowering read named local r{operand} without copying it: {instruction:?}"
                ),
            }
        }
    }
}

/// The invariant a callee laid on its sender's argument slots rests on
/// (`docs/design/compiler.md`, "Sends"): every register above a send's
/// slots is dead when the send executes -- the callee's header and window
/// go over them -- so no instruction after the send reads one before an
/// instruction writes it again, along any path. Both lowerings check it on
/// every body in a debug build, through the leftover analysis, which
/// follows the control flow, back edges and the entry state included.
pub(super) fn check_leftovers(instructions: &[Instruction], local_count: u16, register_count: u16) {
    match leftovers::check(instructions, local_count, register_count) {
        Ok(()) => {}
        Err(LeftoverRefusal::Read {
            instruction,
            register,
        }) => debug_assert!(
            false,
            "internal: the body reads r{register} at instruction {instruction}, which a callee may have left over"
        ),
        Err(LeftoverRefusal::PastTheBound { instruction }) => debug_assert!(
            false,
            "internal: the body passes what the leftover analysis bounds at instruction {instruction}"
        ),
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::cast_possible_truncation,
        reason = "a test's values are small literals and lengths it built"
    )]

    use super::*;
    use indoc::formatdoc;
    use std::path::PathBuf;

    /// Compiles instance-side method `index` of `source` under the reference
    /// lowering, against the bindings the resolver really produces.
    fn compile(source: &str, index: usize) -> CompiledCode {
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
            .expect("a method");
        compile_method(method, &resolved.bindings)
    }

    fn method(declaration: &str) -> String {
        formatdoc! {"
            class X = Object new (
            ) (
              {declaration}
            ) : ()
        "}
    }

    /// Every send the reference lowering emits carries `CallSite::UNCACHED`
    /// (`emit_send`), so these helpers take no id -- there is nothing to vary.
    fn send(
        selector: &str,
        receiver_register: u16,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
    ) -> Instruction {
        Instruction::SendPublic {
            selector: Symbol::intern(selector),
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            call_site: CallSite::UNCACHED,
        }
    }

    fn inlinable(
        selector: &str,
        receiver_register: u16,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
    ) -> Instruction {
        Instruction::SendInlinable {
            selector: Symbol::intern(selector),
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            call_site: CallSite::UNCACHED,
        }
    }

    /// Exactly the sends `control_flow::recognize` accepts are marked, so the
    /// depth count can exempt the block activations the optimized lowering
    /// inlines away (`docs/design/compiler.md`, "How the limit counts").
    #[test]
    fn a_send_the_optimized_lowering_inlines_is_a_send_inlinable() {
        let code = compile(&method("foo: x = ( ^x ifTrue: [1] )"), 0);
        assert!(
            code.instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::SendInlinable { selector, .. } if selector.as_str() == "ifTrue:")),
            "{code:?}"
        );
        assert!(!code
            .instructions()
            .iter()
            .any(|instruction| matches!(instruction, Instruction::SendPublic { .. })));
    }

    #[test]
    fn a_send_the_optimized_lowering_does_not_inline_stays_a_send_public() {
        // A non-literal block argument, and a cascade message: neither is
        // offered to `recognize` in the optimized lowering.
        for declaration in [
            "foo: x = ( ^x ifTrue: x )",
            "foo: x = ( ^x yourself; ifTrue: [1] )",
        ] {
            let code = compile(&method(declaration), 0);
            assert!(
                !code
                    .instructions()
                    .iter()
                    .any(|instruction| matches!(instruction, Instruction::SendInlinable { .. })),
                "{declaration}: {code:?}"
            );
        }
    }

    #[test]
    fn every_inlinable_selector_is_marked() {
        for (declaration, selector) in [
            (
                "foo: x = ( ^x ifTrue: [1] ifFalse: [2] )",
                "ifTrue:ifFalse:",
            ),
            (
                "foo: x = ( ^x ifFalse: [1] ifTrue: [2] )",
                "ifFalse:ifTrue:",
            ),
            ("foo: x = ( ^x ifFalse: [1] )", "ifFalse:"),
            ("foo: x = ( ^x and: [x] )", "and:"),
            ("foo: x = ( ^x or: [x] )", "or:"),
            // A loop's two `value` sends, each the activation of a block the
            // optimized lowering inlines away.
            ("foo: x = ( ^[x] whileTrue: [x] )", "value"),
            ("foo: x = ( ^[x] whileFalse: [x] )", "value"),
        ] {
            let code = compile(&method(declaration), 0);
            assert!(
                code.instructions().iter().any(
                    |instruction| matches!(instruction, Instruction::SendInlinable { selector: inlined_selector, .. } if inlined_selector.as_str() == selector)
                ),
                "{declaration}: {code:?}"
            );
        }
    }

    /// `whileFalse:` is `whileTrue:`'s loop with the exit taken on `true`.
    #[test]
    fn while_false_leaves_its_loop_on_true() {
        let code = compile(&method("foo: x = ( [x] whileFalse: [x] )"), 0);
        let instructions = code.instructions();
        assert!(
            instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::BranchIfTrue { .. })),
            "{code:?}"
        );
        assert!(
            !instructions.iter().any(|instruction| matches!(
                instruction,
                Instruction::BranchIfFalse { .. } | Instruction::SendPublic { .. }
            )),
            "{code:?}"
        );
    }

    /// A loop whose receiver is no block literal is an ordinary send: only
    /// the shape the optimized lowering inlines becomes a loop.
    #[test]
    fn a_loop_over_a_block_in_a_variable_stays_a_send_public() {
        let code = compile(&method("foo: x = ( x whileTrue: [x] )"), 0);
        assert!(no_branch(&code), "{code:?}");
        assert!(
            code.instructions().iter().any(|instruction| matches!(
                instruction,
                Instruction::SendPublic { selector, .. } if selector.as_str() == "whileTrue:"
            )),
            "{code:?}"
        );
    }

    fn closure_code(instruction: &Instruction) -> Rc<CompiledCode> {
        let Instruction::MakeClosure { code, .. } = instruction else {
            panic!("expected a MakeClosure, got {instruction:?}");
        };
        Rc::clone(code)
    }

    fn no_branch(code: &CompiledCode) -> bool {
        code.instructions().iter().all(|instruction| {
            !matches!(
                instruction,
                Instruction::Jump { .. }
                    | Instruction::JumpIfNotBoolean { .. }
                    | Instruction::BranchIfFalse { .. }
                    | Instruction::BranchIfTrue { .. }
                    | Instruction::IntegerBinary { .. }
                    | Instruction::SendMirror { .. }
            )
        })
    }

    /// Compiles `declaration` as the method of a nested class in a
    /// kernel-shaped module that `resolve_kernel` resolved, so that its sends
    /// to `vmMirror` carry `Binding::MirrorSend`.
    fn compile_kernel_method(declaration: &str) -> CompiledCode {
        let source = formatdoc! {"
            class K vmMirror: theMirror = Object new (
              | private vmMirror = theMirror. |
            ) (
              class C = Object new () (
                {declaration}
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
        let nested = resolved
            .module
            .root_class
            .instance_side
            .nested_classes()
            .next()
            .expect("a nested class");
        let method = nested.instance_side.methods().next().expect("a method");
        compile_method(method, &resolved.bindings)
    }

    /// The mirror send the optimized lowering lowers inline is a real send
    /// here, its failure block a real closure, and it is marked
    /// `SendInlinable` so that the depth count exempts the block's
    /// activation as it exempts `ifTrue:`'s.
    #[test]
    fn a_mirror_send_with_a_literal_failure_block_is_a_send_inlinable_over_a_real_closure() {
        let code = compile_kernel_method(
            "foo: argument = ( ^vmMirror string: self concatenate: argument ifFail: [^nil] )",
        );
        assert!(no_branch(&code), "{code:?}");
        assert_eq!(code.instructions().len(), 6, "{code:?}");
        assert!(matches!(
            code.instruction(0),
            Instruction::SendLexical {
                destination: 14,
                ..
            }
        ));
        assert_eq!(
            code.instruction(1),
            Instruction::LoadSelf { destination: 15 }
        );
        assert_eq!(
            code.instruction(2),
            Instruction::Move {
                destination: 16,
                source: 0
            }
        );
        assert!(matches!(
            code.instruction(3),
            Instruction::MakeClosure {
                destination: 17,
                ..
            }
        ));
        assert_eq!(
            code.instruction(4),
            inlinable("string:concatenate:ifFail:", 14, 15, vec![15, 16, 17], 18)
        );
        assert_eq!(code.instruction(5), Instruction::Return { register: 18 });
    }

    /// A failure argument that is not a literal block is not lowered inline
    /// by the optimized lowering, so it is not marked here either.
    #[test]
    fn a_mirror_send_whose_failure_argument_is_not_a_block_stays_a_send_public() {
        let code = compile_kernel_method(
            "foo: argument = ( ^vmMirror stringSize: self ifFail: argument )",
        );
        assert!(code.instructions().iter().any(|instruction| matches!(
            instruction,
            Instruction::SendPublic { selector, .. } if selector.as_str() == "stringSize:ifFail:"
        )));
        assert!(!code
            .instructions()
            .iter()
            .any(|instruction| matches!(instruction, Instruction::SendInlinable { .. })));
    }

    #[test]
    fn reading_a_local_copies_it_into_a_fresh_register() {
        let code = compile(&method("foo: x = ( ^x )"), 0);
        assert_eq!(
            code.instructions(),
            vec![
                Instruction::Move {
                    destination: 14,
                    source: 0
                },
                Instruction::Return { register: 14 }
            ]
        );
        assert_eq!(code.register_count, 15);
    }

    #[test]
    fn no_register_is_reused_across_statements() {
        let code = compile(&method("foo = ( 1. 2 )"), 0);
        assert_eq!(
            code.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 13,
                    literal: BytecodeLiteral::SmallInteger(1)
                },
                Instruction::LoadLiteral {
                    destination: 14,
                    literal: BytecodeLiteral::SmallInteger(2)
                },
                Instruction::LoadSelf { destination: 15 },
                Instruction::ImplicitReturn { register: 15 },
            ]
        );
        assert_eq!(code.statement_program_counter, vec![(0, 0..1), (1, 1..2)]);
        assert_eq!(code.register_count, 16);
    }

    #[test]
    fn if_true_if_false_is_a_real_send_with_two_real_closures() {
        let code = compile(&method("foo: x = ( ^x ifTrue: [1] ifFalse: [2] )"), 0);
        assert!(no_branch(&code));
        assert_eq!(code.instructions().len(), 5);
        assert_eq!(
            code.instruction(0),
            Instruction::Move {
                destination: 14,
                source: 0
            }
        );
        assert!(matches!(
            code.instruction(1),
            Instruction::MakeClosure {
                destination: 15,
                ..
            }
        ));
        assert!(matches!(
            code.instruction(2),
            Instruction::MakeClosure {
                destination: 16,
                ..
            }
        ));
        assert_eq!(
            code.instruction(3),
            inlinable("ifTrue:ifFalse:", 14, 15, vec![15, 16], 17)
        );
        assert_eq!(code.instruction(4), Instruction::Return { register: 17 });
        assert_eq!(
            closure_code(&code.instruction(1)).instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 13,
                    literal: BytecodeLiteral::SmallInteger(1)
                },
                Instruction::ImplicitReturn { register: 13 },
            ]
        );
    }

    #[test]
    fn while_true_is_a_loop_of_value_sends_whose_blocks_reach_the_method_through_their_frames() {
        let code = compile(
            &method("foo = ( | n | n:: 0. [n < 3] whileTrue: [n:: n + 1]. ^n )"),
            0,
        );
        assert_eq!(
            code.instruction(0),
            Instruction::LoadLiteral {
                destination: 14,
                literal: BytecodeLiteral::SmallInteger(0)
            }
        );
        assert_eq!(
            code.instruction(1),
            Instruction::StoreLocal {
                source: 14,
                slot: 13
            }
        );
        assert!(matches!(
            code.instruction(2),
            Instruction::MakeClosure {
                destination: 15,
                ..
            }
        ));
        assert!(matches!(
            code.instruction(3),
            Instruction::MakeClosure {
                destination: 16,
                ..
            }
        ));
        assert_eq!(code.instruction(4), inlinable("value", 15, 17, vec![], 17));
        assert_eq!(
            code.instruction(5),
            Instruction::JumpIfNotBoolean {
                register: 17,
                target: 9
            }
        );
        assert_eq!(
            code.instruction(6),
            Instruction::BranchIfFalse {
                register: 17,
                target: 10
            }
        );
        assert_eq!(code.instruction(7), inlinable("value", 16, 18, vec![], 18));
        assert_eq!(code.instruction(8), Instruction::Jump { target: 4 });
        assert_eq!(
            code.instruction(9),
            Instruction::SignalNonBoolean { register: 17 }
        );
        assert_eq!(
            code.instruction(10),
            Instruction::LoadLiteral {
                destination: 19,
                literal: BytecodeLiteral::Nil
            }
        );
        assert_eq!(
            code.instruction(11),
            Instruction::Move {
                destination: 20,
                source: 13
            }
        );
        assert_eq!(code.instruction(12), Instruction::Return { register: 20 });
        assert_eq!(
            code.statement_program_counter,
            vec![(0, 0..2), (1, 2..11), (2, 11..13)]
        );
        assert_eq!(
            closure_code(&code.instruction(2)).instructions(),
            vec![
                Instruction::LoadOuterLocal {
                    destination: 13,
                    depth: 1,
                    index: 13
                },
                Instruction::LoadLiteral {
                    destination: 14,
                    literal: BytecodeLiteral::SmallInteger(3)
                },
                send("<", 13, 14, vec![14], 15),
                Instruction::ImplicitReturn { register: 15 },
            ]
        );
        assert_eq!(
            closure_code(&code.instruction(3)).instructions(),
            vec![
                Instruction::LoadOuterLocal {
                    destination: 13,
                    depth: 1,
                    index: 13
                },
                Instruction::LoadLiteral {
                    destination: 14,
                    literal: BytecodeLiteral::SmallInteger(1)
                },
                send("+", 13, 14, vec![14], 15),
                Instruction::StoreOuterLocal {
                    source: 15,
                    depth: 1,
                    index: 13
                },
                Instruction::ImplicitReturn { register: 15 },
            ]
        );
    }

    #[test]
    fn a_return_inside_a_block_the_optimized_lowering_inlines_is_a_non_local_return() {
        let code = compile(&method("foo: x = ( x ifTrue: [^1]. ^2 )"), 0);
        let block = closure_code(&code.instruction(1));
        assert_eq!(block.kind, CodeKind::Block);
        assert_eq!(
            block.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 13,
                    literal: BytecodeLiteral::SmallInteger(1)
                },
                Instruction::NonLocalReturn { register: 13 },
            ]
        );
    }

    #[test]
    fn an_assignments_value_is_its_fresh_register_never_the_local() {
        let code = compile(&method("foo = ( | t | ^t:: 3 )"), 0);
        assert_eq!(
            code.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 14,
                    literal: BytecodeLiteral::SmallInteger(3)
                },
                Instruction::StoreLocal {
                    source: 14,
                    slot: 13
                },
                Instruction::Return { register: 14 },
            ]
        );
    }

    #[test]
    fn a_cascade_evaluates_its_receiver_once_and_answers_the_last_send() {
        let code = compile(&method("foo: x = ( ^x foo; bar )"), 0);
        assert_eq!(
            code.instructions(),
            vec![
                Instruction::Move {
                    destination: 14,
                    source: 0
                },
                send("foo", 14, 15, vec![], 15),
                send("bar", 14, 16, vec![], 16),
                Instruction::Return { register: 16 },
            ]
        );
    }

    /// `check_shape` itself: each rule refuses a body that breaks it. One
    /// named local (`r0`), so `r1` is the first scratch register.
    #[test]
    #[should_panic(expected = "read named local r0 without copying it")]
    fn the_shape_check_refuses_a_named_local_read_other_than_by_move() {
        check_shape(&[Instruction::Return { register: 0 }], 1);
    }

    #[test]
    #[should_panic(expected = "copied scratch register r1")]
    fn the_shape_check_refuses_a_move_out_of_a_scratch_register() {
        check_shape(
            &[
                Instruction::LoadSelf { destination: 1 },
                Instruction::Move {
                    destination: 2,
                    source: 1,
                },
            ],
            1,
        );
    }

    #[test]
    #[should_panic(expected = "stored into scratch register r1")]
    fn the_shape_check_refuses_a_store_into_a_scratch_register() {
        check_shape(
            &[
                Instruction::LoadSelf { destination: 2 },
                Instruction::StoreLocal { source: 2, slot: 1 },
            ],
            1,
        );
    }

    /// A loop head reads r3, which the send in the loop's body, laid over
    /// every register from r1 up, left tainted for the next iteration. The
    /// in-order check never saw a read reached only through a back edge.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "internal: the body reads r3")]
    fn a_loop_reading_what_its_body_sent_over_panics_in_debug() {
        check_leftovers(
            &[
                Instruction::LoadSelf { destination: 3 },
                send("foo", 3, 1, vec![], 1),
                Instruction::Jump { target: 1 },
            ],
            1,
            4,
        );
    }

    #[test]
    fn the_shape_check_accepts_a_copied_read_of_a_named_local() {
        check_shape(
            &[
                Instruction::Move {
                    destination: 1,
                    source: 0,
                },
                Instruction::StoreLocal { source: 1, slot: 0 },
                Instruction::Return { register: 1 },
            ],
            1,
        );
    }

    #[test]
    fn an_empty_block_answers_nil() {
        let code = compile(&method("foo = ( ^[] )"), 0);
        assert_eq!(
            closure_code(&code.instruction(0)).instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 13,
                    literal: BytecodeLiteral::Nil
                },
                Instruction::ImplicitReturn { register: 13 },
            ]
        );
    }

    /// The instruction limit, on the counter the reference lowering keeps,
    /// as `lower.rs`'s test of the same name holds the optimized one to it.
    #[test]
    fn a_body_past_the_instruction_limit_is_too_large() {
        let bindings: Bindings = Rc::new(HashMap::new());
        let mut body = Body::new(CodeKind::Method, 0, 0, &bindings);
        let destination = body.fresh();
        let limit = crate::bytecode::INSTRUCTION_LIMIT as usize;
        for _ in 0..limit {
            body.emit(Instruction::LoadSelf { destination });
        }
        assert_eq!(body.too_large, None);
        assert_eq!(body.code.len(), limit);
        body.emit(Instruction::LoadSelf { destination });
        assert_eq!(body.too_large, Some(BodyLimit::Instructions));
        assert_eq!(body.code.len(), limit);
        let code = body.finish(Span::new(0, 0));
        assert_eq!(
            code.instruction(0),
            Instruction::LoadLiteral {
                destination: HEADER_REGISTERS,
                literal: BytecodeLiteral::BodyTooLarge(BodyLimit::Instructions),
            }
        );
        assert_eq!(code.instruction_count(), 2);
    }

    /// The body `compiler::body_too_large` builds for a body of `kind`
    /// taking `argument_count` arguments: its one register is the first
    /// above the header's numbers.
    fn assert_raises_too_large(code: &CompiledCode, kind: CodeKind, argument_count: u16) {
        let register = argument_count + HEADER_REGISTERS;
        assert_eq!(code.kind, kind);
        assert_eq!(
            code.instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: register,
                    literal: BytecodeLiteral::BodyTooLarge(BodyLimit::Registers),
                },
                Instruction::ImplicitReturn { register },
            ]
        );
        assert_eq!(code.argument_count, argument_count);
        assert_eq!(code.register_count, register + 1);
        assert_eq!(code.copied_count, 0);
        assert_eq!(code.statement_program_counter, vec![(0, 0..2)]);
    }

    fn loads_body_too_large(code: &CompiledCode) -> bool {
        code.instructions().iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::LoadLiteral {
                    literal: BytecodeLiteral::BodyTooLarge(_),
                    ..
                }
            )
        })
    }

    /// `count` statements of `1`, each a register of its own.
    fn ones(count: usize) -> String {
        (0..count).map(|_| "1. ").collect()
    }

    /// Every statement costs this lowering a register, and so does the
    /// `self` a method falls off its end with: past the header's numbers,
    /// `u16::MAX - 1 - HEADER_REGISTERS` statements fill the ceiling
    /// exactly, and one more passes it -- which compiles to the raising
    /// body rather than panicking.
    #[test]
    fn a_body_past_the_register_ceiling_compiles_to_one_that_raises() {
        let fitting = usize::from(u16::MAX - 1 - HEADER_REGISTERS);
        let at_the_ceiling = compile(&method(&format!("foo = ( {} )", ones(fitting))), 0);
        assert_eq!(at_the_ceiling.register_count, u16::MAX);
        assert!(!loads_body_too_large(&at_the_ceiling));

        let past = compile(&method(&format!("foo: a = ( {} )", ones(fitting))), 0);
        assert_raises_too_large(&past, CodeKind::Method, 1);
    }

    /// More named temporaries than registers: the body cannot be laid out
    /// at all, and compiles to the raising body.
    #[test]
    fn a_body_with_more_locals_than_registers_compiles_to_one_that_raises() {
        let temporaries: String = (0..=usize::from(u16::MAX))
            .map(|index| format!("t{index} "))
            .collect();
        let code = compile(&method(&format!("foo = ( | {temporaries}| ^t0 )")), 0);
        assert_raises_too_large(&code, CodeKind::Method, 0);
    }

    /// A block is judged on its own: past the header's numbers,
    /// `u16::MAX - HEADER_REGISTERS` statements fill its registers exactly,
    /// since it answers its last statement's register; one more makes only
    /// the block raise, and the method creating it compiles as it would
    /// around any block.
    #[test]
    fn a_block_past_the_register_ceiling_raises_on_its_own() {
        let fitting = usize::from(u16::MAX - HEADER_REGISTERS);
        let at_the_ceiling = compile(&method(&format!("foo = ( ^[ {} ] )", ones(fitting))), 0);
        let block = closure_code(&at_the_ceiling.instruction(0));
        assert_eq!(block.register_count, u16::MAX);
        assert!(!loads_body_too_large(&block));

        let code = compile(&method(&format!("foo = ( ^[ {} ] )", ones(fitting + 1))), 0);
        assert!(!loads_body_too_large(&code));
        assert_raises_too_large(&closure_code(&code.instruction(0)), CodeKind::Block, 0);
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
        let code = compile(&method(&format!("foo = ( ^[{parameters}| p0 ] )")), 0);
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
                    literal: BytecodeLiteral::BodyTooLarge(BodyLimit::Registers),
                },
                Instruction::ImplicitReturn {
                    register: u16::MAX - 1
                },
            ]
        );
    }

    /// A keyword send to `self`, a send to a scratch receiver of a
    /// parameter, a super send of a send's value, and an implicit-receiver
    /// keyword send, each with its first scratch register.
    const ARGUMENT_SLOT_SENDS: &str = indoc::indoc! {"
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

    /// Under the reference lowering every argument is produced in its own
    /// slot, so every source is its slot, and the slots follow the receiver.
    #[test]
    fn every_argument_is_produced_in_its_own_contiguous_slot() {
        for (index, selector, arity, first_scratch) in [
            (4, "foo:bar:", 2, 0),
            (5, "foo:", 1, 1),
            (6, "foo:", 1, 0),
            (7, "foo:", 1, 0),
        ] {
            let code = compile(ARGUMENT_SLOT_SENDS, index);
            let (receiver, argument_base, argument_sources) = code
                .instructions()
                .into_iter()
                .find_map(|instruction| match instruction {
                    Instruction::SendPublic {
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
                .unwrap_or_else(|| panic!("expected a `{selector}` send: {code:?}"));
            assert_eq!(argument_sources.len(), arity, "{code:?}");
            assert!(argument_base >= first_scratch, "{code:?}");
            if let Some(receiver) = receiver {
                assert!(argument_base > receiver, "{code:?}");
            }
            for (offset, source) in argument_sources.iter().enumerate() {
                assert_eq!(
                    *source,
                    argument_base + offset as u16,
                    "every source is its own slot: {code:?}"
                );
            }
        }
    }

    /// Every kind of argument -- a send's value, a literal, a named local,
    /// an outer local, a closure, an inlinable send, an assignment to a
    /// local and through a setter, a cascade, an array -- is produced in its
    /// slot, so the only `Move`s left are the reads of named locals (rule 3).
    #[test]
    fn every_argument_is_lowered_into_its_slot_and_only_named_locals_are_moved() {
        let source = indoc::indoc! {"
            class X = Object new (
            | s |
            ) (
              foo: first baz: second = ( ^first )
              foo: first = ( ^first )
              a = ( ^self )
              straight = ( ^self foo: (a bar) baz: 3 )
              local: x = ( ^self foo: x baz: [x] )
              outer: x = ( ^[self foo: x] )
              branch: c = ( ^self foo: (c ifTrue: [1] ifFalse: [2]) )
              assigns = ( | y | ^self foo: (y:: 3) baz: (s:: 4) )
              cascade = ( ^self foo: (a bar; baz) baz: {1. 2} )
            ) : ()
        "};
        // The method's index and its count of named locals.
        for (index, named_locals) in [(3, 0), (4, 1), (6, 1), (7, 1), (8, 0)] {
            let code = compile(source, index);
            let moves: Vec<Instruction> = code
                .instructions()
                .into_iter()
                .filter(|instruction| matches!(instruction, Instruction::Move { .. }))
                .collect();
            assert!(
                moves.iter().all(|instruction| matches!(
                    instruction,
                    Instruction::Move { source, .. } if *source < named_locals
                )),
                "method {index}: a Move of something other than a named local: {code:?}"
            );
        }
        // The block of `outer:` reads `x` through its frame, into the slot.
        let outer = compile(source, 5);
        let block = outer
            .instructions()
            .into_iter()
            .find_map(|instruction| match instruction {
                Instruction::MakeClosure { code, .. } => Some(code),
                _ => None,
            })
            .expect("a block");
        assert!(
            !block
                .instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Move { .. })),
            "{block:?}"
        );
    }
}
