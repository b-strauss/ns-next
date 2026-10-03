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

use ns_syntax::ast::{self, Expression, Statement};
use ns_syntax::Span;

use super::control_flow;
use super::lower::{lower_literal, selector_of};
use super::simultaneous;
use crate::bytecode::{
    CallSite, CodeBuilder, CodeKind, CompiledCode, Instruction, Literal as BytecodeLiteral,
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
    mixin.initializer_code = Some(Rc::new(initializer(mixin, bindings)));
    mixin.superclass_clause_code = superclass_clause(mixin, bindings).map(Rc::new);
    mixin.superclass_message_code = superclass_message(mixin, bindings).map(Rc::new);
    super::fill_mixin_application_messages(mixin, |mixin, clause| {
        clause_message(mixin, clause, bindings)
    });
}

/// Each own slot's initializer followed by `InitializeSlot`, one statement
/// apiece, then the `initExprs`, then `self`. `InitializeSlot`'s index is the
/// slot's position among the mixin's own slots, never an absolute field index
/// (`docs/design/compiler.md`, "One body per mixin"). A simultaneous slot
/// (§6.3.2) stores its initializer's future provisionally instead, and the
/// clause's end reads each such slot back and stores it resolved, in
/// declaration order.
fn initializer(mixin: &Mixin, bindings: &Bindings) -> CompiledCode {
    let mut body = Body::new(
        CodeKind::Initializer,
        super::factory_argument_count(mixin),
        0,
        bindings,
    );
    let mut source_span: Option<Span> = None;
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
                    own_index: own_index as u32,
                });
            }
            None => {
                let source = body.value(initializer);
                body.emit(Instruction::InitializeSlot {
                    source,
                    own_index: own_index as u32,
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
            own_index: own_index as u32,
        });
        let resolved = body.fresh();
        body.emit(Instruction::ResolveFuture {
            destination: resolved,
            source: slot_value,
        });
        body.emit(Instruction::InitializeSlot {
            source: resolved,
            own_index: own_index as u32,
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
    let statements = [Statement::Return(
        crate::interpreter::nested::superclass_clause_expression(
            clause,
            &declaration.mixin_applications,
        ),
    )];
    let mut body = Body::new(CodeKind::SuperclassClause, 0, 0, bindings);
    body.statements_and_end(&statements);
    Some(body.finish(clause.span))
}

/// The clause's message arguments as a single `^{argument. argument}`, run with
/// the declaring class's own factory arguments.
fn superclass_message(mixin: &Mixin, bindings: &Bindings) -> Option<CompiledCode> {
    let clause = mixin.declaration.as_ref()?.superclass.as_ref()?;
    clause_message(mixin, clause, bindings)
}

/// Any inheritance clause's message arguments -- the superclass clause's or
/// a `<:` clause's -- the same way.
fn clause_message(
    mixin: &Mixin,
    clause: &ast::SuperclassClause,
    bindings: &Bindings,
) -> Option<CompiledCode> {
    let arguments =
        crate::interpreter::nested::superclass_clause_message_arguments_expression(clause)?;
    let mut body = Body::new(
        CodeKind::SuperclassMessage,
        super::factory_argument_count(mixin),
        0,
        bindings,
    );
    body.statements_and_end(&[Statement::Return(arguments)]);
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
    /// The first register above the named locals. Every register from here
    /// on is written by exactly one instruction.
    first_scratch: u16,
    next_register: u16,
    /// Whether the body needs more registers than a `u16` numbers. Set the
    /// moment it is known, and from then on nothing emitted is kept:
    /// `finish` answers `super::body_too_large` instead, never a body whose
    /// register indices wrapped.
    too_large: bool,
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
        let too_large = !super::named_locals_fit(argument_count, temporary_count);
        let first_scratch = if too_large {
            u16::MAX
        } else {
            super::saturated_count(argument_count + temporary_count)
        };
        Body {
            bindings,
            kind,
            argument_count: super::saturated_count(argument_count),
            first_scratch,
            next_register: first_scratch,
            too_large,
            code: CodeBuilder::new(),
            statement_program_counter: Vec::new(),
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
            None => self.too_large = true,
        }
        register
    }

    fn emit(&mut self, instruction: Instruction) {
        self.code.emit(instruction);
    }

    fn binding(&self, span: Span) -> Option<Binding> {
        self.bindings.get(&span).copied()
    }

    fn record_statement(&mut self, start: usize) {
        let index = self.statement_program_counter.len();
        self.statement_program_counter
            .push((index, start..self.code.len()));
    }

    /// Each local slot with an initializer, in declaration order, as one
    /// statement: its expression, then a `StoreLocal` into the slot's own
    /// register, which follows the parameters (§3.6) -- the same shape
    /// `lower.rs`'s `lower_local_initializers` emits, in this module's own
    /// idiom. A simultaneous temporary (§6.3.2) stores its initializer's
    /// future instead, and the clause's end reads each one back and stores
    /// it resolved, in declaration order.
    fn local_initializers(&mut self, temporaries: &[ast::SlotDeclaration]) {
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
            let slot = super::temporary_index(self.argument_count, position);
            self.emit(Instruction::StoreLocal { source, slot });
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
            let start = self.code.len();
            let slot = super::temporary_index(self.argument_count, position);
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
        if self.too_large {
            return;
        }
        let mut last_value = None;
        for statement in statements {
            if self.too_large {
                // Nothing more is kept (`finish`): stop lowering.
                return;
            }
            last_value = self.statement(statement);
        }
        self.end(
            matches!(statements.last(), Some(Statement::Return(_))),
            last_value,
        );
    }

    /// One of the body's own statements, recorded in
    /// `statement_program_counter`. Answers the statement's value register, or
    /// `None` when it emitted a return.
    fn statement(&mut self, statement: &Statement) -> Option<u16> {
        let start = self.code.len();
        let value = match statement {
            Statement::Expression(expression) => Some(self.value(expression)),
            Statement::Return(expression) => {
                let register = self.value(expression);
                match self.kind {
                    CodeKind::Method | CodeKind::SuperclassClause | CodeKind::SuperclassMessage => {
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
            CodeKind::Block | CodeKind::SuperclassClause | CodeKind::SuperclassMessage
                if !ends_in_return =>
            {
                let register = match last_value {
                    Some(register) => register,
                    None => self.literal(BytecodeLiteral::Nil),
                };
                self.emit(Instruction::ImplicitReturn { register });
            }
            CodeKind::Method
            | CodeKind::Block
            | CodeKind::SuperclassClause
            | CodeKind::SuperclassMessage => {}
        }
    }

    fn return_self(&mut self) {
        let register = self.fresh();
        self.emit(Instruction::LoadSelf {
            destination: register,
        });
        self.emit(Instruction::ImplicitReturn { register });
    }

    fn literal(&mut self, literal: BytecodeLiteral) -> u16 {
        let destination = self.fresh();
        self.emit(Instruction::LoadLiteral {
            destination,
            literal,
        });
        destination
    }

    /// Lowers `expression` and answers the register holding its value -- always
    /// a register this lowering wrote for it and will never write again.
    fn value(&mut self, expression: &Expression) -> u16 {
        match expression {
            Expression::Literal(literal) => self.literal(lower_literal(literal)),
            Expression::Identifier(id) => self.read(id),
            Expression::Assign { target, value, .. } => self.assign(target, value),
            Expression::Send(send) => self.send(send),
            Expression::Cascade(cascade) => self.cascade(cascade),
            Expression::Block(block) => self.block(block),
            Expression::DynamicArray { elements, .. } => {
                let element_registers = self.values(elements);
                let destination = self.fresh();
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

    fn read(&mut self, id: &ast::Identifier) -> u16 {
        if id.name == "self" {
            let destination = self.fresh();
            self.emit(Instruction::LoadSelf { destination });
            return destination;
        }
        match self.binding(id.span) {
            Some(Binding::Local(source)) => {
                let destination = self.fresh();
                self.emit(Instruction::Move {
                    destination,
                    source,
                });
                destination
            }
            Some(Binding::OuterLocal { depth, index }) => {
                let destination = self.fresh();
                self.emit(Instruction::LoadOuterLocal {
                    destination,
                    depth,
                    index,
                });
                destination
            }
            Some(Binding::LexicalSend { depth }) => {
                let selector = Symbol::intern(&id.name);
                self.emit_send(|destination, call_site| Instruction::SendLexical {
                    depth,
                    selector,
                    argument_registers: vec![],
                    destination,
                    call_site,
                })
            }
            // A `super`-prefixed inheritance-clause receiver, `super Foo`:
            // §6.1's super send of `Foo` from the class declaring the clause.
            Some(Binding::SuperSend) => {
                let selector = Symbol::intern(&id.name);
                self.emit_send(|destination, call_site| Instruction::SendSuper {
                    selector,
                    argument_registers: vec![],
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
                let destination = self.fresh();
                self.emit(Instruction::LoadLiteral {
                    destination,
                    literal: BytecodeLiteral::UnresolvedIdentifier(id.name.clone()),
                });
                destination
            }
        }
    }

    /// The assignment's value is the register its value was computed into,
    /// never the local it was stored in.
    fn assign(&mut self, target: &ast::Identifier, value: &Expression) -> u16 {
        let value_register = self.value(value);
        match self.binding(target.span) {
            Some(Binding::Local(slot)) => self.emit(Instruction::StoreLocal { source: value_register, slot }),
            Some(Binding::OuterLocal { depth, index }) => {
                self.emit(Instruction::StoreOuterLocal { source: value_register, depth, index });
            }
            Some(Binding::LexicalSend { depth }) => {
                let selector = Symbol::intern(&format!("{}:", target.name));
                self.emit_send(|destination, call_site| Instruction::SendLexical {
                    depth,
                    selector,
                    argument_registers: vec![value_register],
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

    fn send(&mut self, send: &ast::Send) -> u16 {
        let selector = selector_of(&send.message);
        match self.binding(send.span) {
            Some(Binding::EnclosingObject { depth }) => {
                let destination = self.fresh();
                self.emit(Instruction::LoadEnclosingObject { destination, depth });
                destination
            }
            Some(Binding::SuperSend) => {
                let argument_registers = self.values(&send.message.arguments);
                self.emit_send(|destination, call_site| Instruction::SendSuper {
                    selector,
                    argument_registers,
                    destination,
                    call_site,
                })
            }
            Some(Binding::LexicalSend { depth }) => {
                let argument_registers = self.values(&send.message.arguments);
                self.emit_send(|destination, call_site| Instruction::SendLexical {
                    depth,
                    selector,
                    argument_registers,
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
                let receiver_register = self.value(&send.receiver);
                if control_flow::recognize(send).is_some()
                    || control_flow::recognize_mirror_send(self.bindings, send).is_some()
                {
                    self.inlinable_send(receiver_register, &send.message)
                } else {
                    self.public_send(receiver_register, &send.message)
                }
            }
        }
    }

    fn public_send(&mut self, receiver_register: u16, message: &ast::MessageSend) -> u16 {
        let selector = selector_of(message);
        let argument_registers = self.values(&message.arguments);
        self.emit_send(|destination, call_site| Instruction::SendPublic {
            selector,
            receiver_register,
            argument_registers,
            destination,
            call_site,
        })
    }

    fn inlinable_send(&mut self, receiver_register: u16, message: &ast::MessageSend) -> u16 {
        let selector = selector_of(message);
        let argument_registers = self.values(&message.arguments);
        self.emit_send(|destination, call_site| Instruction::SendInlinable {
            selector,
            receiver_register,
            argument_registers,
            destination,
            call_site,
        })
    }

    fn cascade(&mut self, cascade: &ast::Cascade) -> u16 {
        let receiver_register = self.value(&cascade.receiver);
        let mut result = self.public_send(receiver_register, &cascade.first);
        for message in &cascade.rest {
            result = self.public_send(receiver_register, message);
        }
        result
    }

    fn block(&mut self, block: &ast::BlockExpression) -> u16 {
        let parameters: Vec<Symbol> = block
            .parameters
            .iter()
            .map(|parameter| Symbol::intern(&parameter.name))
            .collect();
        let mut inner = Body::new(
            CodeKind::Block,
            parameters.len(),
            block.body.temporaries.len(),
            self.bindings,
        );
        inner.local_initializers(&block.body.temporaries);
        inner.statements_and_end(&block.body.statements);
        let code = inner.finish(block.span);
        let parameters = super::parameters_of(parameters, &code);
        let destination = self.fresh();
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
    fn emit_send(&mut self, build: impl FnOnce(u16, CallSite) -> Instruction) -> u16 {
        let destination = self.fresh();
        self.emit(build(destination, CallSite::UNCACHED));
        destination
    }

    fn finish(self, source: Span) -> CompiledCode {
        if self.too_large {
            return super::body_too_large(self.kind, self.argument_count, source);
        }
        if cfg!(debug_assertions) {
            check_shape(&self.code.instructions(), self.first_scratch);
        }
        CompiledCode::from_builder(
            self.kind,
            self.code,
            self.argument_count,
            self.next_register,
            0,
            self.statement_program_counter,
            source,
        )
    }
}

/// The shape the reference lowering promises (`docs/design/compiler.md`,
/// "What it emits"), checked on every body in a debug build, which is what
/// the test suite runs: no branch, since only inlining emits one; every
/// register above the named locals written by exactly one instruction; a
/// named local written only by `StoreLocal`, which `destination` does not
/// count; and a named local read only by `Move`, every other operand being a
/// scratch register. The last is rule 3, "every read of a local is copied",
/// which is what keeps a value from changing under its consumer -- the
/// property the optimized lowering has to earn with copies of its own
/// ("The value-register invariant").
fn check_shape(instructions: &[Instruction], first_scratch: u16) {
    let mut written = HashSet::new();
    for instruction in instructions {
        assert!(
            !matches!(
                instruction,
                Instruction::Jump { .. }
                    | Instruction::JumpIfNotBoolean { .. }
                    | Instruction::BranchIfFalse { .. }
                    | Instruction::BranchIfTrue { .. }
                    | Instruction::IntegerBinary { .. }
            ),
            "internal: the reference lowering emitted a branch: {instruction:?}"
        );
        if let Some(written_register) = destination(instruction) {
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
        for operand in operands(instruction) {
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

/// The registers of this body that `instruction` reads. Exhaustive on purpose,
/// like `destination`: a new instruction has to decide here what it reads. The
/// `index` of an outer-local access names a register of another frame, so it is
/// not one of them.
fn operands(instruction: &Instruction) -> Vec<u16> {
    match instruction {
        Instruction::Move { source, .. }
        | Instruction::StoreLocal { source, .. }
        | Instruction::StoreOuterLocal { source, .. }
        | Instruction::InitializeSlot { source, .. }
        | Instruction::InitializeSlotProvisionally { source, .. } => vec![*source],
        Instruction::MakeFuture { closure, .. } => vec![*closure],
        Instruction::ResolveFuture { source, .. } => vec![*source],
        Instruction::Return { register }
        | Instruction::ImplicitReturn { register }
        | Instruction::NonLocalReturn { register }
        | Instruction::JumpIfNotBoolean { register, .. }
        | Instruction::BranchIfFalse { register, .. }
        | Instruction::BranchIfTrue { register, .. } => vec![*register],
        Instruction::SendPublic {
            receiver_register,
            argument_registers,
            ..
        }
        | Instruction::SendInlinable {
            receiver_register,
            argument_registers,
            ..
        } => std::iter::once(*receiver_register)
            .chain(argument_registers.iter().copied())
            .collect(),
        Instruction::SendLexical {
            argument_registers, ..
        }
        | Instruction::SendSuper {
            argument_registers, ..
        } => argument_registers.clone(),
        Instruction::MakeArray {
            element_registers, ..
        } => element_registers.clone(),
        // Unreachable in reference-lowered code: closure conversion, which
        // is what emits these, does not run under the reference lowering.
        // Named here anyway, so a real fifth-instruction addition still has
        // to say what it reads.
        Instruction::LoadVector { vector, .. } => vec![*vector],
        Instruction::StoreVector { vector, source, .. } => vec![*vector, *source],
        // Unreachable in reference-lowered code: the reference lowering emits
        // only the ordinary send for every integer leaf (`docs/design/compiler.md`,
        // "The reference lowering"), never this instruction. Named here anyway,
        // so a real addition still has to say what it reads.
        Instruction::IntegerBinary { left, right, .. } => vec![*left, *right],
        // Unreachable in reference-lowered code, for the same reason: the
        // reference lowering sends a mirror send as `SendInlinable`, its
        // failure block a real closure. Named so an addition still decides.
        Instruction::SendMirror {
            mirror_register,
            subject_register,
            argument_registers,
            ..
        } => [*mirror_register, *subject_register]
            .into_iter()
            .chain(argument_registers.iter().copied())
            .collect(),
        Instruction::LoadLiteral { .. }
        | Instruction::LoadSelf { .. }
        | Instruction::LoadEnclosingObject { .. }
        | Instruction::LoadOuterLocal { .. }
        | Instruction::MakeClosure { .. }
        | Instruction::MakeVector { .. }
        | Instruction::ReturnSlotIfFilled { .. }
        | Instruction::LoadOwnSlot { .. }
        | Instruction::Jump { .. } => vec![],
    }
}

/// The register `instruction` writes, if any. Exhaustive on purpose: a new
/// instruction has to decide here whether it writes a register.
fn destination(instruction: &Instruction) -> Option<u16> {
    match instruction {
        Instruction::LoadLiteral { destination, .. }
        | Instruction::LoadSelf { destination }
        | Instruction::LoadEnclosingObject { destination, .. }
        | Instruction::Move { destination, .. }
        | Instruction::LoadOuterLocal { destination, .. }
        | Instruction::SendLexical { destination, .. }
        | Instruction::SendPublic { destination, .. }
        | Instruction::SendInlinable { destination, .. }
        | Instruction::SendSuper { destination, .. }
        | Instruction::MakeClosure { destination, .. }
        | Instruction::MakeArray { destination, .. }
        | Instruction::MakeVector { destination, .. }
        | Instruction::LoadVector { destination, .. }
        | Instruction::IntegerBinary { destination, .. }
        | Instruction::SendMirror { destination, .. }
        | Instruction::MakeFuture { destination, .. }
        | Instruction::ResolveFuture { destination, .. }
        | Instruction::LoadOwnSlot { destination, .. } => Some(*destination),
        Instruction::StoreLocal { .. }
        | Instruction::StoreOuterLocal { .. }
        | Instruction::StoreVector { .. }
        | Instruction::InitializeSlot { .. }
        | Instruction::ReturnSlotIfFilled { .. }
        | Instruction::InitializeSlotProvisionally { .. }
        | Instruction::Return { .. }
        | Instruction::ImplicitReturn { .. }
        | Instruction::NonLocalReturn { .. }
        | Instruction::Jump { .. }
        | Instruction::JumpIfNotBoolean { .. }
        | Instruction::BranchIfFalse { .. }
        | Instruction::BranchIfTrue { .. } => None,
    }
}

#[cfg(test)]
mod tests {
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
        compile_method(
            &resolved.module.root_class.instance_side.methods[index],
            &resolved.bindings,
        )
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
        argument_registers: Vec<u16>,
        destination: u16,
    ) -> Instruction {
        Instruction::SendPublic {
            selector: Symbol::intern(selector),
            receiver_register,
            argument_registers,
            destination,
            call_site: CallSite::UNCACHED,
        }
    }

    fn inlinable(
        selector: &str,
        receiver_register: u16,
        argument_registers: Vec<u16>,
        destination: u16,
    ) -> Instruction {
        Instruction::SendInlinable {
            selector: Symbol::intern(selector),
            receiver_register,
            argument_registers,
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
            ("foo: x = ( ^[x] whileTrue: [x] )", "whileTrue:"),
            ("foo: x = ( ^[x] whileFalse: [x] )", "whileFalse:"),
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
        compile_method(
            &resolved.module.root_class.instance_side.nested_classes[0]
                .instance_side
                .methods[0],
            &resolved.bindings,
        )
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
            Instruction::SendLexical { destination: 1, .. }
        ));
        assert_eq!(
            code.instruction(1),
            Instruction::LoadSelf { destination: 2 }
        );
        assert_eq!(
            code.instruction(2),
            Instruction::Move {
                destination: 3,
                source: 0
            }
        );
        assert!(matches!(
            code.instruction(3),
            Instruction::MakeClosure { destination: 4, .. }
        ));
        assert_eq!(
            code.instruction(4),
            inlinable("string:concatenate:ifFail:", 1, vec![2, 3, 4], 5)
        );
        assert_eq!(code.instruction(5), Instruction::Return { register: 5 });
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
                    destination: 1,
                    source: 0
                },
                Instruction::Return { register: 1 }
            ]
        );
        assert_eq!(code.register_count, 2);
    }

    #[test]
    fn no_register_is_reused_across_statements() {
        let code = compile(&method("foo = ( 1. 2 )"), 0);
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
                Instruction::LoadSelf { destination: 2 },
                Instruction::ImplicitReturn { register: 2 },
            ]
        );
        assert_eq!(code.statement_program_counter, vec![(0, 0..1), (1, 1..2)]);
        assert_eq!(code.register_count, 3);
    }

    #[test]
    fn if_true_if_false_is_a_real_send_with_two_real_closures() {
        let code = compile(&method("foo: x = ( ^x ifTrue: [1] ifFalse: [2] )"), 0);
        assert!(no_branch(&code));
        assert_eq!(code.instructions().len(), 5);
        assert_eq!(
            code.instruction(0),
            Instruction::Move {
                destination: 1,
                source: 0
            }
        );
        assert!(matches!(
            code.instruction(1),
            Instruction::MakeClosure { destination: 2, .. }
        ));
        assert!(matches!(
            code.instruction(2),
            Instruction::MakeClosure { destination: 3, .. }
        ));
        assert_eq!(
            code.instruction(3),
            inlinable("ifTrue:ifFalse:", 1, vec![2, 3], 4)
        );
        assert_eq!(code.instruction(4), Instruction::Return { register: 4 });
        assert_eq!(
            closure_code(&code.instruction(1)).instructions(),
            vec![
                Instruction::LoadLiteral {
                    destination: 0,
                    literal: BytecodeLiteral::SmallInteger(1)
                },
                Instruction::ImplicitReturn { register: 0 },
            ]
        );
    }

    #[test]
    fn while_true_is_a_real_send_whose_blocks_reach_the_method_through_their_frames() {
        let code = compile(
            &method("foo = ( | n | n:: 0. [n < 3] whileTrue: [n:: n + 1]. ^n )"),
            0,
        );
        assert!(no_branch(&code));
        assert_eq!(
            code.instruction(0),
            Instruction::LoadLiteral {
                destination: 1,
                literal: BytecodeLiteral::SmallInteger(0)
            }
        );
        assert_eq!(
            code.instruction(1),
            Instruction::StoreLocal { source: 1, slot: 0 }
        );
        assert!(matches!(
            code.instruction(2),
            Instruction::MakeClosure { destination: 2, .. }
        ));
        assert!(matches!(
            code.instruction(3),
            Instruction::MakeClosure { destination: 3, .. }
        ));
        assert_eq!(code.instruction(4), inlinable("whileTrue:", 2, vec![3], 4));
        assert_eq!(
            code.instruction(5),
            Instruction::Move {
                destination: 5,
                source: 0
            }
        );
        assert_eq!(code.instruction(6), Instruction::Return { register: 5 });
        assert_eq!(
            code.statement_program_counter,
            vec![(0, 0..2), (1, 2..5), (2, 5..7)]
        );
        assert_eq!(
            closure_code(&code.instruction(2)).instructions(),
            vec![
                Instruction::LoadOuterLocal {
                    destination: 0,
                    depth: 1,
                    index: 0
                },
                Instruction::LoadLiteral {
                    destination: 1,
                    literal: BytecodeLiteral::SmallInteger(3)
                },
                send("<", 0, vec![1], 2),
                Instruction::ImplicitReturn { register: 2 },
            ]
        );
        assert_eq!(
            closure_code(&code.instruction(3)).instructions(),
            vec![
                Instruction::LoadOuterLocal {
                    destination: 0,
                    depth: 1,
                    index: 0
                },
                Instruction::LoadLiteral {
                    destination: 1,
                    literal: BytecodeLiteral::SmallInteger(1)
                },
                send("+", 0, vec![1], 2),
                Instruction::StoreOuterLocal {
                    source: 2,
                    depth: 1,
                    index: 0
                },
                Instruction::ImplicitReturn { register: 2 },
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
                    destination: 0,
                    literal: BytecodeLiteral::SmallInteger(1)
                },
                Instruction::NonLocalReturn { register: 0 },
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
                    destination: 1,
                    literal: BytecodeLiteral::SmallInteger(3)
                },
                Instruction::StoreLocal { source: 1, slot: 0 },
                Instruction::Return { register: 1 },
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
                    destination: 1,
                    source: 0
                },
                send("foo", 1, vec![], 2),
                send("bar", 1, vec![], 3),
                Instruction::Return { register: 3 },
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
                    destination: 0,
                    literal: BytecodeLiteral::Nil
                },
                Instruction::ImplicitReturn { register: 0 },
            ]
        );
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
        code.instructions().iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::LoadLiteral {
                    literal: BytecodeLiteral::BodyTooLarge,
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
    /// `self` a method falls off its end with: `u16::MAX - 1` statements
    /// fill the ceiling exactly, and one more passes it -- which compiles to
    /// the raising body rather than panicking.
    #[test]
    fn a_body_past_the_register_ceiling_compiles_to_one_that_raises() {
        let at_the_ceiling = compile(
            &method(&format!("foo = ( {} )", ones(usize::from(u16::MAX) - 1))),
            0,
        );
        assert_eq!(at_the_ceiling.register_count, u16::MAX);
        assert!(!loads_body_too_large(&at_the_ceiling));

        let past = compile(
            &method(&format!("foo: a = ( {} )", ones(usize::from(u16::MAX)))),
            0,
        );
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

    /// A block is judged on its own: `u16::MAX` statements fill its
    /// registers exactly, since it answers its last statement's register;
    /// one more makes only the block raise, and the method creating it
    /// compiles as it would around any block.
    #[test]
    fn a_block_past_the_register_ceiling_raises_on_its_own() {
        let at_the_ceiling = compile(
            &method(&format!("foo = ( ^[ {} ] )", ones(usize::from(u16::MAX)))),
            0,
        );
        let block = closure_code(&at_the_ceiling.instruction(0));
        assert_eq!(block.register_count, u16::MAX);
        assert!(!loads_body_too_large(&block));

        let code = compile(
            &method(&format!(
                "foo = ( ^[ {} ] )",
                ones(usize::from(u16::MAX) + 1)
            )),
            0,
        );
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
                    literal: BytecodeLiteral::BodyTooLarge,
                },
                Instruction::ImplicitReturn {
                    register: u16::MAX - 1
                },
            ]
        );
    }
}
