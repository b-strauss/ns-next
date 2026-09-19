//! `Expression`/`Statement` -> `Instruction` lowering, register allocation, and
//! the scope stack that maps resolver depths onto runtime frame hops. See
//! `docs/design/compiler.md`'s "Registers" and "Control-flow inlining"
//! sections.
//!
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use ns_syntax::ast::{self, Expression, Statement};
use ns_syntax::Span;

use super::control_flow::{self, InlineForm};
use crate::bytecode::{CallSite, CodeKind, CompiledCode, Instruction, Literal as BytecodeLiteral};
use crate::interpreter::literals::parse_integer_literal;
use crate::types::Binding;

/// One entry of the compile-time scope stack. The resolver counts every
/// block literal as one scope; at run time only `Frame` scopes cost a
/// `captured_frame` hop -- an `Inlined` scope (control-flow inlining) has
/// no frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeKind {
    Frame,
    Inlined,
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

/// The runtime `captured_frame` hop count for a resolver depth of `depth`
/// scopes: the number of `Frame` scopes among the innermost `depth`
/// entries of `scopes` (innermost last).
pub(crate) fn hops_for_depth(scopes: &[ScopeKind], depth: u16) -> u16 {
    scopes
        .iter()
        .rev()
        .take(depth as usize)
        .filter(|scope| **scope == ScopeKind::Frame)
        .count() as u16
}

pub(crate) struct Lowerer<'a> {
    bindings: &'a Rc<HashMap<Span, Binding>>,
    kind: CodeKind,
    argument_count: u16,
    /// Innermost last. The last entry is this body's own `Frame` scope
    /// with `Inlined` entries pushed above it while an inlined block body
    /// is being lowered.
    scopes: Vec<ScopeKind>,
    instructions: Vec<Instruction>,
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
}

impl<'a> Lowerer<'a> {
    pub(crate) fn new(
        kind: CodeKind,
        argument_count: u16,
        named_temporary_count: u16,
        bindings: &'a Rc<HashMap<Span, Binding>>,
        mut enclosing_scopes: Vec<ScopeKind>,
        first_call_site: u32,
    ) -> Self {
        enclosing_scopes.push(ScopeKind::Frame);
        let base = argument_count + named_temporary_count;
        Lowerer {
            bindings,
            kind,
            argument_count,
            scopes: enclosing_scopes,
            instructions: Vec::new(),
            base,
            next_scratch: base,
            max_register: base,
            statement_index: 0,
            statement_program_counter: Vec::new(),
            next_call_site: first_call_site,
            last_expression_register: None,
        }
    }

    fn allocate_scratch(&mut self) -> u16 {
        let register = self.next_scratch;
        self.next_scratch += 1;
        self.max_register = self.max_register.max(self.next_scratch);
        register
    }

    fn emit(&mut self, instruction: Instruction) -> usize {
        self.instructions.push(instruction);
        self.instructions.len() - 1
    }

    /// Lowers a body's own statements, then its end (`finish_body`). The
    /// last statement's value is read only when falling off the end answers
    /// it; every other statement's value is discarded.
    pub(crate) fn lower_body(&mut self, statements: &[Statement]) {
        let last_use = if self.answers_last_value() {
            ValueUse::Read
        } else {
            ValueUse::Discarded
        };
        for (index, statement) in statements.iter().enumerate() {
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
        let start = self.instructions.len();
        self.lower_statement_inner(statement, use_);
        let end = self.instructions.len();
        self.statement_program_counter
            .push((self.statement_index, start..end));
        self.statement_index += 1;
    }

    /// The initializer's per-slot statement: the slot's initializer expression,
    /// then `InitializeSlot`, which writes the field directly, never through a
    /// setter send. One `statement_program_counter` entry. `own_index` is the
    /// slot's position among the mixin's own slots (declaration order) -- the
    /// VM turns it into an absolute field index at run time via the executing
    /// frame's `slot_base`.
    pub(crate) fn lower_slot_initializer(&mut self, initializer: &Expression, own_index: u32) {
        self.next_scratch = self.base;
        let start = self.instructions.len();
        let source = self.lower_expression_for(initializer, ValueUse::Read);
        self.emit(Instruction::InitializeSlot { source, own_index });
        let end = self.instructions.len();
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
    pub(crate) fn lower_local_initializers(&mut self, temporaries: &[ast::SlotDeclaration]) {
        for (position, temporary) in temporaries.iter().enumerate() {
            let Some(initializer) = &temporary.initializer else {
                continue;
            };
            self.next_scratch = self.base;
            let start = self.instructions.len();
            let source = self.lower_expression_for(initializer, ValueUse::Read);
            let slot = self.argument_count + position as u16;
            self.emit(Instruction::StoreLocal { source, slot });
            let end = self.instructions.len();
            self.statement_program_counter
                .push((self.statement_index, start..end));
            self.statement_index += 1;
        }
    }

    /// Lowers a statement without touching the allocator or the PC table --
    /// shared by `lower_statement` and `lower_inline_block_body`. `use_` is
    /// what reads an expression statement's value; a `^` that returns reads
    /// its own. Returns the value register for a `Statement::Expression` (or a
    /// discarded initializer `^`), `None` when a return instruction was
    /// emitted.
    fn lower_statement_inner(&mut self, statement: &Statement, use_: ValueUse) -> Option<u16> {
        match statement {
            Statement::Expression(expression) => {
                let register = self.lower_expression_for(expression, use_);
                self.last_expression_register = Some(register);
                Some(register)
            }
            Statement::Return(expression) => {
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
            // Either clause body is a single synthetic `^expression` (the
            // receiver, or the message's argument list), so its top-level
            // `^` is what hands that value back; a `^` written inside a
            // block of the clause's message returns from the message's own
            // activation, which catches it (`Mutator::catches_return`),
            // exactly as a method's activation would.
            (CodeKind::SuperclassClause | CodeKind::SuperclassMessage, false) => {
                Some(|register| Instruction::Return { register })
            }
            (CodeKind::SuperclassClause | CodeKind::SuperclassMessage, true) => {
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
            CodeKind::Block | CodeKind::SuperclassClause | CodeKind::SuperclassMessage => true,
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
        let ends_in_return = matches!(statements.last(), Some(Statement::Return(_)))
            && self.kind != CodeKind::Initializer;
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
    fn lower_block_literal(&mut self, block: &ast::BlockExpression) -> u16 {
        let parameters: Vec<crate::Symbol> = block
            .parameters
            .iter()
            .map(|parameter| crate::Symbol::intern(&parameter.name))
            .collect();
        let mut inner = Lowerer::new(
            CodeKind::Block,
            parameters.len() as u16,
            block.body.temporaries.len() as u16,
            self.bindings,
            self.scopes.clone(),
            self.next_call_site,
        );
        inner.lower_local_initializers(&block.body.temporaries);
        inner.lower_body(&block.body.statements);
        // The block's sends share the enclosing family's one counter, so the
        // root and every nested block number densely across the whole
        // family, never restarting at each `MakeClosure`
        // (`docs/design/dispatch.md`, "Code families").
        let after = inner.next_call_site;
        let code = inner.finish(block.span);
        self.next_call_site = after;
        let destination = self.allocate_scratch();
        self.emit(Instruction::MakeClosure {
            destination,
            parameters: Rc::new(parameters),
            code: Rc::new(code),
        });
        destination
    }

    /// Copies `register` into a fresh scratch register when it is one of this
    /// frame's *named* locals and `clobbered` says something lowered
    /// between here and the consuming instruction can write it.
    ///
    /// `lower_expression`'s contract is that the register it returns still
    /// holds that value when its consumer executes -- every consumer defers the
    /// read to instruction-execution time. A named local's register (`< base`)
    /// is live storage, so an operand that has to survive a write to it is
    /// copied out first. A register `>= base` needs no copy: no `Binding` can
    /// name it, so only this body's own instructions ever write it, and the
    /// scratch allocator never reuses a live one.
    fn materialize(&mut self, register: u16, clobbered: bool) -> u16 {
        if !clobbered || register >= self.base {
            return register;
        }
        let destination = self.allocate_scratch();
        self.emit(Instruction::Move {
            destination,
            source: register,
        });
        destination
    }

    /// Lowers one operand of a multi-operand instruction. `rest` is
    /// everything lowered after this operand but before that instruction
    /// executes -- the arguments still to come, for instance.
    fn lower_operand(&mut self, expression: &Expression, rest: &[Expression]) -> u16 {
        let clobbered = rest
            .iter()
            .any(|expression| self.may_clobber_locals(expression));
        let register = self.lower_expression(expression);
        self.materialize(register, clobbered)
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
    /// very next instruction (`^x := y`, with a setter that assigns `y`).
    /// Only when nothing reads the value at all is the copy pure overhead.
    ///
    /// Every position that knows its use routes through here and states it:
    /// a body's statements (`lower_body`), a `^`, a slot initializer, an
    /// inlined block's statements, and the inlined forms that pass their own
    /// use on to their blocks. None of them infers it, so a new position
    /// cannot silently be given the cheap form without the property holding.
    fn lower_expression_for(&mut self, expression: &Expression, use_: ValueUse) -> u16 {
        match (use_, expression) {
            (ValueUse::Discarded, Expression::Assign { target, value, .. }) => {
                self.lower_assign(target, value, ValueUse::Discarded)
            }
            (ValueUse::Discarded, Expression::Send(send)) => {
                self.lower_send(send, ValueUse::Discarded)
            }
            _ => self.lower_expression(expression),
        }
    }

    /// Whether lowering `expression` can emit an instruction that writes one of
    /// this frame's named locals: a `StoreLocal`/`StoreOuterLocal` from an
    /// assignment, or *any* send -- a send runs user code, and that code can
    /// assign this frame's locals through a closure that captured the frame.
    /// The closure need not appear in `expression` at all (an earlier statement
    /// may have built and stored it), so every send counts.
    ///
    /// Conservative in one direction only: answering `true` costs one
    /// `Move`, answering `false` when a write can happen breaks the
    /// value-register invariant (`docs/design/compiler.md`).
    fn may_clobber_locals(&self, expression: &Expression) -> bool {
        match expression {
            // `LoadLiteral` and `MakeClosure` allocate; neither runs user
            // code nor writes a named local.
            Expression::Literal(_) | Expression::Block(_) => false,
            Expression::Identifier(id) => match self.bindings.get(&id.span) {
                // A register read (`LoadOuterLocal` at worst).
                Some(Binding::Local(_) | Binding::OuterLocal { .. }) => false,
                // An implicit receiverless send -- real dispatch.
                Some(Binding::LexicalSend { .. }) => true,
                // `self`, a bare builtin class name, and the deferred
                // unresolved-identifier case: a single register load each.
                Some(Binding::SuperSend) | None => false,
            },
            Expression::Assign { .. } | Expression::Send(_) | Expression::Cascade(_) => true,
            // `MakeArray` itself cannot; its elements can.
            Expression::DynamicArray { elements, .. } => elements
                .iter()
                .any(|element| self.may_clobber_locals(element)),
        }
    }

    /// Returns the register holding `expression`'s value.
    pub(crate) fn lower_expression(&mut self, expression: &Expression) -> u16 {
        match expression {
            Expression::Literal(literal) => {
                let destination = self.allocate_scratch();
                self.emit(Instruction::LoadLiteral {
                    destination,
                    literal: lower_literal(literal),
                });
                destination
            }
            Expression::Identifier(id) => self.lower_identifier_read(id),
            Expression::Assign { target, value, .. } => {
                self.lower_assign(target, value, ValueUse::Read)
            }
            Expression::Send(send) => self.lower_send(send, ValueUse::Read),
            Expression::Cascade(cascade) => self.lower_cascade(cascade),
            Expression::Block(block) => self.lower_block_literal(block),
            Expression::DynamicArray { elements, .. } => {
                let element_registers: Vec<u16> = (0..elements.len())
                    .map(|index| self.lower_operand(&elements[index], &elements[index + 1..]))
                    .collect();
                let destination = self.allocate_scratch();
                self.emit(Instruction::MakeArray {
                    destination,
                    element_registers,
                });
                destination
            }
        }
    }

    /// Maps a resolver `Binding::Local`/`OuterLocal` onto the runtime
    /// register file: `(hops, index)`, where `hops == 0` means "register
    /// `index` of the current frame".
    fn local_target(&self, depth: u16, index: u16) -> (u16, u16) {
        if depth == 0 {
            assert!(
                self.scopes.last() == Some(&ScopeKind::Frame),
                "internal: Binding::Local inside an inlined block (the inlining gate admits no names there)"
            );
        }
        (hops_for_depth(&self.scopes, depth), index)
    }

    fn lower_identifier_read(&mut self, id: &ast::Identifier) -> u16 {
        if id.name == "self" {
            let destination = self.allocate_scratch();
            self.emit(Instruction::LoadSelf { destination });
            return destination;
        }
        match self.bindings.get(&id.span) {
            Some(Binding::Local(slot)) => {
                let (_, index) = self.local_target(0, *slot);
                index
            }
            Some(Binding::OuterLocal { depth, index }) => {
                let (hops, index) = self.local_target(*depth, *index);
                if hops == 0 {
                    return index;
                }
                let destination = self.allocate_scratch();
                self.emit(Instruction::LoadOuterLocal {
                    destination,
                    depth: hops,
                    index,
                });
                destination
            }
            Some(Binding::LexicalSend { depth }) => {
                let depth = *depth;
                self.emit_lexical_send(depth, crate::Symbol::intern(&id.name), vec![])
            }
            // The resolver attaches `SuperSend` to a bare identifier in one
            // place: a `super`-prefixed superclass-clause receiver (the known
            // gap in `docs/design/dispatch.md`, "Binding variants"). It is
            // grouped with `None` because both fall to the same builtin-name
            // fallback.
            Some(Binding::SuperSend) | None => {
                match crate::interpreter::instantiate::builtin_kind_from_name(&id.name) {
                    Some(kind) => {
                        let destination = self.allocate_scratch();
                        self.emit(Instruction::LoadBuiltinClass { destination, kind });
                        destination
                    }
                    // Not a compiler-invariant violation: `resolve_identifier`
                    // deliberately records no binding for `super`, and the
                    // lexer deliberately does not reserve it, so `^super`
                    // is a program the language accepts and rejects at
                    // *evaluation* time. Compilation cannot raise a
                    // `NewspeakError`, so the error is deferred exactly like
                    // `MalformedInteger`: the VM raises `literals`' own error
                    // when the instruction executes.
                    None => {
                        let destination = self.allocate_scratch();
                        self.emit(Instruction::LoadLiteral {
                            destination,
                            literal: BytecodeLiteral::UnresolvedIdentifier(id.name.clone()),
                        });
                        destination
                    }
                }
            }
        }
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
        // A setter send runs user code before anything reads this expression's
        // own value, so a named local's register may no longer hold what was
        // assigned by then. (The `StoreLocal`/`StoreOuterLocal` cases below
        // need no copy: the only named local a `StoreLocal` can write while
        // `value_register` is live is `value_register` itself -- `t := t` --
        // which leaves it unchanged, and a `StoreOuterLocal` writes a different
        // frame.)
        if use_ == ValueUse::Read
            && matches!(
                self.bindings.get(&target.span),
                Some(Binding::LexicalSend { .. })
            )
        {
            value_register = self.materialize(value_register, true);
        }
        match self.bindings.get(&target.span) {
            Some(Binding::Local(slot)) => {
                let (_, index) = self.local_target(0, *slot);
                self.emit(Instruction::StoreLocal {
                    source: value_register,
                    slot: index,
                });
            }
            Some(Binding::OuterLocal { depth, index }) => {
                let (hops, index) = self.local_target(*depth, *index);
                if hops == 0 {
                    self.emit(Instruction::StoreLocal {
                        source: value_register,
                        slot: index,
                    });
                } else {
                    self.emit(Instruction::StoreOuterLocal {
                        source: value_register,
                        depth: hops,
                        index,
                    });
                }
            }
            Some(Binding::LexicalSend { depth }) => {
                let depth = *depth;
                let setter = crate::Symbol::intern(&format!("{}:", target.name));
                self.emit_lexical_send(depth, setter, vec![value_register]);
            }
            Some(Binding::SuperSend) | None => panic!(
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

    fn emit_send(&mut self, build: impl FnOnce(u16, CallSite) -> Instruction) -> u16 {
        let destination = self.allocate_scratch();
        let call_site = self.take_call_site();
        self.emit(build(destination, call_site));
        destination
    }

    fn emit_lexical_send(
        &mut self,
        depth: u16,
        selector: crate::Symbol,
        argument_registers: Vec<u16>,
    ) -> u16 {
        self.emit_send(|destination, call_site| Instruction::SendLexical {
            depth,
            selector,
            argument_registers,
            destination,
            call_site,
        })
    }

    pub(crate) fn emit_ordinary_send_with_receiver(
        &mut self,
        receiver_register: u16,
        message: &ast::MessageSend,
    ) -> u16 {
        let selector = selector_of(message);
        let argument_registers = self.lower_arguments(message);
        self.emit_send(|destination, call_site| Instruction::SendPublic {
            selector,
            receiver_register,
            argument_registers,
            destination,
            call_site,
        })
    }

    /// Every argument is live until the send instruction reads it, so each
    /// one has to survive whatever the arguments after it emit.
    fn lower_arguments(&mut self, message: &ast::MessageSend) -> Vec<u16> {
        (0..message.arguments.len())
            .map(|index| {
                self.lower_operand(&message.arguments[index], &message.arguments[index + 1..])
            })
            .collect()
    }

    /// `use_` is what reads the send's value. Only an inlined form passes it
    /// on, to the blocks whose values become its own.
    fn lower_send(&mut self, send: &ast::Send, use_: ValueUse) -> u16 {
        match self.bindings.get(&send.span) {
            Some(Binding::SuperSend) => {
                let selector = selector_of(&send.message);
                let argument_registers = self.lower_arguments(&send.message);
                self.emit_send(|destination, call_site| Instruction::SendSuper {
                    selector,
                    argument_registers,
                    destination,
                    call_site,
                })
            }
            Some(Binding::LexicalSend { depth }) => {
                let depth = *depth;
                let selector = selector_of(&send.message);
                let argument_registers = self.lower_arguments(&send.message);
                self.emit_lexical_send(depth, selector, argument_registers)
            }
            _ => {
                // The inlining gate: only an ordinary explicit-receiver send
                // is ever offered to `recognize` (`docs/design/compiler.md`,
                // "Control-flow inlining"). Every recognized form still keeps
                // a runtime guard whose failure path is a real virtual send,
                // so inlining stays a pure optimization.
                if let Some(form) = control_flow::recognize(send) {
                    return match form {
                        InlineForm::IfTrueIfFalse(true_block, false_block) => {
                            self.lower_if(send, Some(true_block), Some(false_block), use_)
                        }
                        InlineForm::IfTrue(true_block) => {
                            self.lower_if(send, Some(true_block), None, use_)
                        }
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
                // The receiver is read by the send instruction, so it has to
                // survive its own arguments' lowering.
                let receiver_register = self.lower_operand(&send.receiver, &send.message.arguments);
                self.emit_ordinary_send_with_receiver(receiver_register, &send.message)
            }
        }
    }

    fn lower_cascade(&mut self, cascade: &ast::Cascade) -> u16 {
        // The one receiver register is read again by every message of the
        // cascade, so it has to survive the earlier *sends* too, not just
        // the arguments lowered between them.
        let clobbered = !cascade.rest.is_empty()
            || cascade
                .first
                .arguments
                .iter()
                .any(|argument| self.may_clobber_locals(argument));
        let receiver_register = self.lower_expression(&cascade.receiver);
        let receiver_register = self.materialize(receiver_register, clobbered);
        let mut result = self.emit_ordinary_send_with_receiver(receiver_register, &cascade.first);
        for message in &cascade.rest {
            result = self.emit_ordinary_send_with_receiver(receiver_register, message);
        }
        result
    }

    /// The index the next emitted instruction will get -- a forward label.
    fn here(&self) -> u32 {
        self.instructions.len() as u32
    }

    fn patch_target(&mut self, instruction_index: usize, target: u32) {
        match &mut self.instructions[instruction_index] {
            Instruction::Jump {
                target: jump_target,
            }
            | Instruction::JumpIfNotBoolean {
                target: jump_target,
                ..
            }
            | Instruction::BranchIfFalse {
                target: jump_target,
                ..
            }
            | Instruction::BranchIfTrue {
                target: jump_target,
                ..
            } => *jump_target = target,
            other => unreachable!("internal: patch_target on a non-jump instruction {other:?}"),
        }
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

    /// `builtins::boolean::truthy`'s rule in bytecode: only exactly
    /// `Boolean(true)` is true; a non-`true` `Boolean` or a non-`Boolean` is
    /// false.
    fn emit_coerce_to_boolean(&mut self, source: u16) -> u16 {
        let destination = self.allocate_scratch();
        let not_boolean_jump = self.emit(Instruction::JumpIfNotBoolean {
            register: source,
            target: 0,
        });
        let false_jump = self.emit(Instruction::BranchIfFalse {
            register: source,
            target: 0,
        });
        self.emit(Instruction::LoadLiteral {
            destination,
            literal: BytecodeLiteral::Boolean(true),
        });
        let after_true_jump = self.emit(Instruction::Jump { target: 0 });
        let false_label = self.here();
        self.patch_target(not_boolean_jump, false_label);
        self.patch_target(false_jump, false_label);
        self.emit(Instruction::LoadLiteral {
            destination,
            literal: BytecodeLiteral::Boolean(false),
        });
        let end_label = self.here();
        self.patch_target(after_true_jump, end_label);
        destination
    }

    /// The guard-failure path of an inlined loop: what the native
    /// `whileTrue:`/`whileFalse:` does with a non-Boolean condition value.
    fn emit_must_be_boolean(&mut self, register: u16) -> u16 {
        let selector = crate::Symbol::MUST_BE_BOOLEAN;
        self.emit_send(|destination, call_site| Instruction::SendPublic {
            selector,
            receiver_register: register,
            argument_registers: vec![],
            destination,
            call_site,
        })
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
    /// branch is `nil`, matching `builtins::boolean`'s one-armed arms. A
    /// branch's value is only moved into the send's own result, so `use_`
    /// -- what reads that result -- is what reads the branch's value.
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
            |lowerer| lowerer.emit_ordinary_send_with_receiver(guard_register, &send.message),
        );
        destination
    }

    /// `and:`/`or:`: the short-circuit arm is the receiver's own Boolean
    /// value, the other arm the block's value coerced by `truthy`'s rule.
    /// The coercion only chooses which literal becomes the send's result, so
    /// `use_` -- what reads that result -- is what reads the block's value.
    fn lower_and_or(
        &mut self,
        send: &ast::Send,
        block: &ast::BlockExpression,
        is_and: bool,
        use_: ValueUse,
    ) -> u16 {
        let guard_register = self.lower_expression(&send.receiver);
        let destination = self.allocate_scratch();
        let evaluate_and_coerce = |lowerer: &mut Self| {
            let block_register = lowerer.lower_inline_block_body(block, use_);
            lowerer.emit_coerce_to_boolean(block_register)
        };
        let fallback = |lowerer: &mut Self| {
            lowerer.emit_ordinary_send_with_receiver(guard_register, &send.message)
        };
        if is_and {
            self.emit_guarded_branch(
                guard_register,
                destination,
                evaluate_and_coerce,
                |_| guard_register,
                fallback,
            );
        } else {
            self.emit_guarded_branch(
                guard_register,
                destination,
                |_| guard_register,
                evaluate_and_coerce,
                fallback,
            );
        }
        destination
    }

    /// A real loop: condition, guard, exit branch, body, back-edge. A
    /// non-Boolean condition value takes the `mustBeBoolean` path and ends the
    /// loop with that send's value; a normal exit leaves `nil` in
    /// `destination`. The guard reads the condition's value on every iteration,
    /// whatever reads the loop's; nothing reads the body's.
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
        let fallback_result = self.emit_must_be_boolean(guard_register);
        self.emit(Instruction::Move {
            destination,
            source: fallback_result,
        });
        let end_label = self.here();
        self.patch_target(exit_jump, end_label);
        destination
    }

    pub(crate) fn finish(self, source: Span) -> CompiledCode {
        // Control-flow inlining is the only source of jumps, and every one of
        // its labels is a `here()` that `finish_body`'s terminator guarantees
        // is a real instruction index, so a target must be in range. This
        // check does *not* catch a never-patched target: an unpatched
        // `target` field stays `0`, which is always in range and would pass
        // silently here. Completeness against that failure mode instead
        // comes from each emitter's own emit/patch pairing (every `emit(...
        // { target: 0 })` in this module is paired with a `patch_target`
        // call before `finish` runs) -- an out-of-range target is a compiler
        // bug, not a program error, so it is a `debug_assert!` -- checked on
        // every debug test run, free in release.
        debug_assert!(
            self.instructions
                .iter()
                .filter_map(|instruction| match instruction {
                    Instruction::Jump { target }
                    | Instruction::JumpIfNotBoolean { target, .. }
                    | Instruction::BranchIfFalse { target, .. }
                    | Instruction::BranchIfTrue { target, .. } => Some(*target as usize),
                    _ => None,
                })
                .all(|target| target < self.instructions.len()),
            "internal: a jump target does not name an instruction of this body"
        );
        CompiledCode {
            kind: self.kind,
            family_size: crate::bytecode::family_size_of(&self.instructions),
            instructions: self.instructions,
            argument_count: self.argument_count,
            register_count: self.max_register,
            statement_program_counter: self.statement_program_counter,
            source,
        }
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
        // Parsed once, here, with `parse_integer_literal`, the parser the VM
        // re-runs for a `MalformedInteger`. A literal it rejects is a
        // Newspeak-program error the language raises at evaluation time, so
        // it is deferred (`MalformedInteger`), never a compile-time panic.
        ast::Literal::Integer { text, .. } => match parse_integer_literal(text) {
            Ok(integer) => BytecodeLiteral::SmallInteger(integer),
            Err(_) => BytecodeLiteral::MalformedInteger(text.clone()),
        },
        ast::Literal::Float { text, .. } => {
            BytecodeLiteral::Float(text.parse().unwrap_or(f64::NAN))
        }
        ast::Literal::String { value, .. } => BytecodeLiteral::String(value.clone()),
        ast::Literal::Symbol { value, .. } => BytecodeLiteral::Symbol(crate::Symbol::intern(value)),
        ast::Literal::Character { value, .. } => BytecodeLiteral::Character(*value),
        ast::Literal::Boolean { value, .. } => BytecodeLiteral::Boolean(*value),
        ast::Literal::Nil { .. } => BytecodeLiteral::Nil,
        ast::Literal::Array { elements, .. } => {
            BytecodeLiteral::Array(elements.iter().map(lower_literal).collect())
        }
        ast::Literal::Pattern { .. } => BytecodeLiteral::Nil, // patterns are not implemented (no typechecker)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::{formatdoc, indoc};
    use std::path::PathBuf;

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
        let method = resolved.module.root_class.instance_side.methods[index].clone();
        (method, resolved.bindings.clone())
    }

    pub(super) fn method_lowerer<'a>(
        method: &ast::MethodDeclaration,
        bindings: &'a Rc<HashMap<Span, Binding>>,
    ) -> Lowerer<'a> {
        let argument_count = match &method.pattern {
            ast::MessagePattern::Unary { .. } => 0,
            ast::MessagePattern::Binary { .. } => 1,
            ast::MessagePattern::Keyword { parts } => parts.len() as u16,
        };
        Lowerer::new(
            CodeKind::Method,
            argument_count,
            method.body.temporaries.len() as u16,
            bindings,
            vec![],
            0,
        )
    }

    fn return_expression(method: &ast::MethodDeclaration) -> &Expression {
        let Statement::Return(expression) = &method.body.statements[0] else {
            panic!("expected a return statement");
        };
        expression
    }

    #[test]
    fn a_literal_returns_a_fresh_scratch_register() {
        let (method, bindings) = resolve_method(
            indoc! {"
                Newspeak3
                'T'
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
            code.instructions,
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
                Newspeak3
                'T'
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
            code.instructions.is_empty(),
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
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo: x bar: y = ( ^x )
                  m = ( | t | t := 1. ^self foo: t bar: (t := 9) )
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
        let Some(Instruction::SendLexical {
            argument_registers, ..
        }) = code
            .instructions
            .iter()
            .find(|instruction| matches!(instruction, Instruction::SendLexical { .. }))
        else {
            panic!("expected the keyword send");
        };
        assert_ne!(
            argument_registers[0], 0,
            "`t` is register 0; the first argument must not be its live register"
        );
        assert!(
            code.instructions.contains(&Instruction::Move {
                destination: argument_registers[0],
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
                Newspeak3
                'T'
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
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Move { .. })),
            "no operand needs a copy here: {:?}",
            code.instructions
        );
        assert_eq!(
            code.instructions
                .iter()
                .filter_map(|instruction| match instruction {
                    Instruction::SendPublic {
                        receiver_register,
                        argument_registers,
                        ..
                    } => Some((*receiver_register, argument_registers.clone())),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![(0, vec![1, 2])],
            "receiver and first argument are read straight out of their own registers"
        );
    }

    /// A bare `super` is a program-level error the language raises at
    /// evaluation time, not a compiler-invariant violation, so it must lower
    /// to the deferred literal rather than panic.
    #[test]
    fn a_bare_super_lowers_to_a_deferred_error_literal_not_a_panic() {
        let (method, bindings) = resolve_method(
            indoc! {"
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo = ( ^super )
                ) : ()
            "},
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings);
        lowerer.lower_expression(return_expression(&method));
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instructions,
            vec![Instruction::LoadLiteral {
                destination: 0,
                literal: BytecodeLiteral::UnresolvedIdentifier("super".to_string()),
            }]
        );
    }

    #[test]
    fn assigning_a_named_temporary_emits_store_local_and_records_the_statement_range() {
        let (method, bindings) = resolve_method(
            indoc! {"
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo = ( | t | t := 7. ^t )
                ) : ()
            "},
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings); // temporary `t` is register 0, base 1
        lowerer.lower_statement(&method.body.statements[0], ValueUse::Discarded); // `t := 7.`
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instructions,
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
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo = ( | t | t := 1. t := 2. ^t )
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
            code.instructions,
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
    fn an_out_of_range_integer_literal_lowers_to_malformed_integer_not_a_panic() {
        let (method, bindings) = resolve_method(
            indoc! {"
                Newspeak3
                'T'
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
            code.instructions,
            vec![Instruction::LoadLiteral {
                destination: 0,
                literal: BytecodeLiteral::MalformedInteger("99999999999999999999".to_string()),
            }]
        );
    }

    #[test]
    fn a_radix_literal_is_parsed_at_compile_time() {
        let (method, bindings) = resolve_method(
            indoc! {"
                Newspeak3
                'T'
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
            code.instructions,
            vec![Instruction::LoadLiteral {
                destination: 0,
                literal: BytecodeLiteral::SmallInteger(255)
            }]
        );
    }

    #[test]
    fn an_ordinary_send_evaluates_receiver_then_arguments() {
        let (method, bindings) = resolve_method(
            indoc! {"
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo: x = ( ^3 + x )
                ) : ()
            "},
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings);
        let destination = lowerer.lower_expression(return_expression(&method));
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instructions,
            vec![
                Instruction::LoadLiteral {
                    destination: 1,
                    literal: BytecodeLiteral::SmallInteger(3)
                },
                Instruction::SendPublic {
                    selector: crate::Symbol::intern("+"),
                    receiver_register: 1,
                    argument_registers: vec![0],
                    destination: 2,
                    call_site: CallSite { number: 0 },
                },
            ]
        );
        assert_eq!(destination, 2);
        assert_eq!(code.register_count, 3);
        assert_eq!(code.family_size, 1);
    }

    #[test]
    fn a_bare_implicit_send_lowers_to_send_lexical() {
        // `foo`'s body (`^bar`) resolves `bar` as an implicit-receiver unary
        // send at depth 0 (`bar` is a sibling method) -- `Binding::LexicalSend`.
        let source = indoc! {"
            Newspeak3
            'T'
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
            code.instructions,
            vec![Instruction::SendLexical {
                depth: 0,
                selector: crate::Symbol::intern("bar"),
                argument_registers: vec![],
                destination: 0,
                call_site: CallSite { number: 0 },
            }]
        );
        assert_eq!(destination, 0);
    }

    #[test]
    fn a_keyword_implicit_send_and_a_super_send_lower_without_a_receiver_register() {
        let source = indoc! {"
            Newspeak3
            'T'
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
            &code.instructions[2],
            Instruction::SendLexical { depth: 0, selector, argument_registers, .. }
                if selector.as_str() == "at:put:" && argument_registers.len() == 2
        ));
        let (foo, bindings) = resolve_method(source, 1);
        let mut lowerer = method_lowerer(&foo, &bindings);
        lowerer.lower_expression(match &foo.body.statements[1] {
            Statement::Return(expression) => expression,
            _ => panic!(),
        });
        let code = lowerer.finish(foo.span);
        assert!(matches!(
            &code.instructions[0],
            Instruction::SendSuper { selector, argument_registers, destination: 0, .. }
                if selector.as_str() == "printString" && argument_registers.is_empty()
        ));
    }

    #[test]
    fn a_slot_assignment_lowers_to_a_setter_lexical_send() {
        let source = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
              | n ::= 0. |
            ) (
              bump = ( n := n + 1 )
            ) : ()
        "};
        let (bump, bindings) = resolve_method(source, 0);
        let mut lowerer = method_lowerer(&bump, &bindings);
        lowerer.lower_statement(&bump.body.statements[0], ValueUse::Discarded);
        let code = lowerer.finish(bump.span);
        assert!(matches!(
            code.instructions.last(),
            Some(Instruction::SendLexical { depth: 0, selector, argument_registers, .. })
                if selector.as_str() == "n:" && argument_registers.len() == 1
        ));
    }

    /// The argument register of every `n:` setter send in `code`, in order.
    fn setter_arguments(code: &CompiledCode) -> Vec<u16> {
        code.instructions
            .iter()
            .filter_map(|instruction| match instruction {
                Instruction::SendLexical {
                    selector,
                    argument_registers,
                    ..
                } if selector.as_str() == "n:" => Some(argument_registers[0]),
                _ => None,
            })
            .collect()
    }

    /// The copy `lower_assign` makes for a setter send protects the
    /// assignment's *value* from the user code that setter runs, which can
    /// write the local through a closure before anything reads the value --
    /// even a consumer that is the very next instruction. Only where nothing
    /// reads the value is the copy pure overhead: one instruction, and one more
    /// register in every activation's `temporaries`. This test pins the cheap
    /// form for a statement; the tests after it pin the copy for each position
    /// that reads the value, and `ns-cli`'s
    /// `an_assignment_through_a_setter_answers_the_value_before_the_setter_ran`
    /// pins what a program observes.
    #[test]
    fn a_slot_assignment_in_statement_position_does_not_copy_its_value() {
        let source = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
              | n ::= 0. |
            ) (
              m = ( | t | t := 1. n := t )
            ) : ()
        "};
        let code = lower_whole_method(source, 0); // temporary `t` is register 0
        assert!(
            !code
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Move { .. })),
            "nothing reads the assignment's value, so it needs no copy: {:?}",
            code.instructions
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
            Newspeak3
            'T'
            class X = Object new (
              | n ::= 0. |
            ) (
              m = ( | t | t := 1. ^n := t )
            ) : ()
        "};
        let code = lower_whole_method(source, 0); // temporary `t` is register 0
        let [copy] = setter_arguments(&code)[..] else {
            panic!("expected one setter send: {:?}", code.instructions);
        };
        assert_ne!(copy, 0, "the setter can write `t` before `Return` reads");
        assert!(code.instructions.contains(&Instruction::Move {
            destination: copy,
            source: 0
        }));
        assert_eq!(
            code.instructions.last(),
            Some(&Instruction::Return { register: copy })
        );
    }

    #[test]
    fn a_slot_initializer_that_assigns_a_slot_copies_its_value_out_of_the_local() {
        // A method body stands in for the initializer expression, as in
        // `a_slot_initializer_is_one_statement_ending_in_initialize_slot`; the
        // parameter gives the assignment a named local to hand on.
        let source = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
              | n ::= 0. |
            ) (
              foo: p = ( ^n := p )
            ) : ()
        "};
        let (method, bindings) = resolve_method(source, 0);
        let mut lowerer = Lowerer::new(CodeKind::Initializer, 1, 0, &bindings, vec![], 0);
        lowerer.lower_slot_initializer(return_expression(&method), 4);
        let code = lowerer.finish(method.span);
        let [copy] = setter_arguments(&code)[..] else {
            panic!("expected one setter send: {:?}", code.instructions);
        };
        assert_ne!(
            copy, 0,
            "the setter can write `p` before `InitializeSlot` reads"
        );
        assert_eq!(
            code.instructions,
            vec![
                Instruction::Move {
                    destination: copy,
                    source: 0
                },
                Instruction::SendLexical {
                    depth: 0,
                    selector: crate::Symbol::intern("n:"),
                    argument_registers: vec![copy],
                    destination: copy + 1,
                    call_site: CallSite { number: 0 },
                },
                Instruction::InitializeSlot {
                    source: copy,
                    own_index: 4
                },
            ]
        );
    }

    /// A block answers its last statement's value, so that statement's
    /// assignment is copied; an earlier statement's is not.
    #[test]
    fn a_blocks_last_slot_assignment_copies_its_value_and_an_earlier_one_does_not() {
        let source = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
              | n ::= 0. |
            ) (
              m = ( ^[:t | n := t. n := t] )
            ) : ()
        "};
        let code = lower_whole_method(source, 0);
        let Instruction::MakeClosure { code: block, .. } = &code.instructions[0] else {
            panic!("expected the block literal: {:?}", code.instructions);
        };
        let [discarded, read] = setter_arguments(block)[..] else {
            panic!("expected two setter sends: {:?}", block.instructions);
        };
        assert_eq!(
            discarded, 0,
            "the first value is discarded: `t`'s own register"
        );
        assert_ne!(read, 0, "the block answers the second value");
        assert!(block.instructions.contains(&Instruction::Move {
            destination: read,
            source: 0
        }));
        assert_eq!(
            block.instructions.last(),
            Some(&Instruction::ImplicitReturn { register: read })
        );
    }

    /// An inlined block's last statement is read exactly when the inlined
    /// form's own value is -- a loop condition always, a loop body never.
    #[test]
    fn an_inlined_blocks_last_slot_assignment_is_copied_only_when_its_value_is_read() {
        let source = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
              | n ::= 0. |
            ) (
              discarded: c = ( | t | c ifTrue: [n := t] ifFalse: [n := t]. c and: [n := t] )
              returned: c = ( | t | ^c ifTrue: [n := t] ifFalse: [n := t] )
              returnedAnd: c = ( | t | ^c and: [n := t] )
              loop = ( | t | [n := t] whileTrue: [n := t] )
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
                returned.instructions
            );
        }
        let loop_code = lower_whole_method(source, 3);
        let [condition, body] = setter_arguments(&loop_code)[..] else {
            panic!("expected two setter sends: {:?}", loop_code.instructions);
        };
        assert_ne!(condition, 0, "the guard reads the condition's value");
        assert_eq!(body, 0, "nothing reads the loop body's value");
    }

    #[test]
    fn a_bare_builtin_class_name_lowers_to_load_builtin_class() {
        let source = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              foo = ( ^OrderedCollection )
            ) : ()
        "};
        let (foo, bindings) = resolve_method(source, 0);
        let mut lowerer = method_lowerer(&foo, &bindings);
        let destination = lowerer.lower_expression(return_expression(&foo));
        let code = lowerer.finish(foo.span);
        assert_eq!(
            code.instructions,
            vec![Instruction::LoadBuiltinClass {
                destination: 0,
                kind: crate::types::BuiltinClass::OrderedCollection
            }]
        );
        assert_eq!(destination, 0);
    }

    #[test]
    fn a_cascade_reuses_one_receiver_register_across_every_message() {
        let source = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              foo = ( | c | c := OrderedCollection new. c add: 1; add: 2 )
            ) : ()
        "};
        let (foo, bindings) = resolve_method(source, 0);
        let mut lowerer = method_lowerer(&foo, &bindings); // temporary `c` is register 0
        lowerer.lower_statement(&foo.body.statements[0], ValueUse::Discarded);
        lowerer.lower_statement(&foo.body.statements[1], ValueUse::Discarded);
        let code = lowerer.finish(foo.span);
        let receiver_registers: Vec<u16> = code
            .instructions
            .iter()
            .filter_map(|instruction| match instruction {
                Instruction::SendPublic {
                    receiver_register, ..
                } => Some(*receiver_register),
                _ => None,
            })
            .collect();
        // `OrderedCollection new`, then the cascade's two `add:` sends.
        assert_eq!(receiver_registers.len(), 3);
        // `c` is register 0, but the cascade's receiver has to survive the
        // sends between the messages -- any of which can reassign `c` --
        // so it is copied into a scratch register first.
        assert!(
            receiver_registers[1] >= 1,
            "the cascade receiver must not be `c`'s own live register"
        );
        assert!(
            code.instructions.contains(&Instruction::Move {
                destination: receiver_registers[1],
                source: 0
            }),
            "the copy out of `c` is a Move"
        );
        assert_eq!(
            receiver_registers[1], receiver_registers[2],
            "both cascade sends share one receiver register"
        );
        assert_eq!(code.family_size, 3);
    }

    #[test]
    fn hops_count_only_frame_scopes_within_the_depth() {
        use ScopeKind::*;
        // [Frame, Inlined, Frame] innermost-last: from the innermost frame,
        // resolver depth 2 (block -> inlined -> method) crosses one frame.
        assert_eq!(hops_for_depth(&[Frame, Inlined, Frame], 2), 1);
        // From directly inside an inlined block, depth 1 is the enclosing
        // frame's own registers: zero hops.
        assert_eq!(hops_for_depth(&[Frame, Inlined], 1), 0);
        // No inlining anywhere: hops == depth.
        assert_eq!(hops_for_depth(&[Frame, Frame, Frame], 2), 2);
        assert_eq!(hops_for_depth(&[Frame], 0), 0);
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
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo = ( 1 + 1 )
                ) : ()
            "},
            0,
        );
        assert_eq!(
            &code.instructions[code.instructions.len() - 2..],
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
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo = ( ^1 )
                ) : ()
            "},
            0,
        );
        assert_eq!(
            code.instructions,
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
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo = ( ^[ :x | x ] )
                ) : ()
            "},
            0,
        );
        let Some(Instruction::MakeClosure {
            destination: 0,
            parameters,
            code: block,
        }) = code.instructions.first()
        else {
            panic!(
                "expected a MakeClosure instruction, got {:?}",
                code.instructions
            );
        };
        assert_eq!(parameters.as_slice(), &[crate::Symbol::intern("x")]);
        assert_eq!(block.kind, CodeKind::Block);
        assert_eq!(block.argument_count, 1);
        // `x` is the block's own register 0: read with no instruction, then
        // returned as the block's implicit value.
        assert_eq!(
            block.instructions,
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
            Newspeak3
            'T'
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
            code.instructions
                .iter()
                .filter_map(Instruction::call_site)
                .map(|site| site.number)
                .collect()
        };
        let first_block = |code: &CompiledCode| -> Rc<CompiledCode> {
            code.instructions
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
        // `ns-next disasm` on START when the plan was written.
        assert_eq!(numbers(&code), vec![0, 4, 5]);
        assert_eq!(code.family_size, 6);
        let block = first_block(&code);
        assert_eq!(numbers(&block), vec![1, 3]);
        assert_eq!(block.family_size, 4);
        let inner = first_block(&block);
        assert_eq!(numbers(&inner), vec![2]);
        assert_eq!(inner.family_size, 3);
    }

    #[test]
    fn a_block_reads_and_writes_enclosing_locals_through_outer_local_instructions() {
        let code = lower_whole_method(
            indoc! {"
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo: x = ( | t | ^[ t := x ] )
                ) : ()
            "},
            0,
        );
        let Some(Instruction::MakeClosure { code: block, .. }) = code.instructions.first() else {
            panic!()
        };
        assert_eq!(
            block.instructions,
            vec![
                Instruction::LoadOuterLocal {
                    destination: 0,
                    depth: 1,
                    index: 0
                }, // x
                Instruction::StoreOuterLocal {
                    source: 0,
                    depth: 1,
                    index: 1
                }, // t
                Instruction::ImplicitReturn { register: 0 },
            ]
        );
    }

    #[test]
    fn a_doubly_nested_block_counts_two_frame_hops() {
        let code = lower_whole_method(
            indoc! {"
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo: x = ( ^[ [ x ] ] )
                ) : ()
            "},
            0,
        );
        let Some(Instruction::MakeClosure { code: outer, .. }) = code.instructions.first() else {
            panic!()
        };
        let Some(Instruction::MakeClosure { code: inner, .. }) = outer.instructions.first() else {
            panic!()
        };
        assert_eq!(
            inner.instructions[0],
            Instruction::LoadOuterLocal {
                destination: 0,
                depth: 2,
                index: 0
            }
        );
    }

    #[test]
    fn a_return_inside_a_block_is_a_non_local_return() {
        let code = lower_whole_method(
            indoc! {"
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo = ( ^[ ^1 ] )
                ) : ()
            "},
            0,
        );
        let Some(Instruction::MakeClosure { code: block, .. }) = code.instructions.first() else {
            panic!()
        };
        assert_eq!(
            block.instructions,
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
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo = ( [ ] value. ^[ 1. 2 ] value )
                ) : ()
            "},
            0,
        );
        let Some(Instruction::MakeClosure { code: empty, .. }) = code.instructions.first() else {
            panic!()
        };
        assert_eq!(
            empty.instructions,
            vec![
                Instruction::LoadLiteral {
                    destination: 0,
                    literal: BytecodeLiteral::Nil
                },
                Instruction::ImplicitReturn { register: 0 }
            ]
        );
        let Some(Instruction::MakeClosure { code: two, .. }) = code.instructions.get(2) else {
            panic!("{:?}", code.instructions)
        };
        assert_eq!(
            two.instructions,
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
                Newspeak3
                'T'
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
            code.instructions,
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
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo = ( ^1 )
                ) : ()
            "},
            0,
        );
        // Reuse `foo`'s `^1` as if it were an initializer expression: kind Initializer, 0 arguments.
        let mut lowerer = Lowerer::new(CodeKind::Initializer, 0, 0, &bindings, vec![], 0);
        lowerer.lower_body(&method.body.statements);
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instructions,
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
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo = ( ^6 * 7 )
                ) : ()
            "},
            0,
        );
        let mut lowerer = Lowerer::new(CodeKind::Initializer, 0, 0, &bindings, vec![], 0);
        lowerer.lower_slot_initializer(return_expression(&method), 4);
        let code = lowerer.finish(method.span);
        // `6`, `7`, the `*` send, then the field write: four instructions.
        assert!(matches!(
            code.instructions.last(),
            Some(Instruction::InitializeSlot {
                source: 2,
                own_index: 4
            })
        ));
        assert_eq!(code.statement_program_counter, vec![(0, 0..4)]);
    }

    /// Compiles `foo: x = ( ^<body> )` and returns the method's code.
    fn compile_foo(body: &str) -> CompiledCode {
        let full = formatdoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              foo: x = ( ^{body} )
            ) : ()
        "};
        lower_whole_method(&full, 0)
    }

    fn has<F: Fn(&Instruction) -> bool>(code: &CompiledCode, predicate: F) -> bool {
        code.instructions.iter().any(predicate)
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
            code.instructions
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
            code.instructions
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
        let code = compile_foo("x ifTrue: [ | t | t := 1. t ] ifFalse: [2]");
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
    fn while_true_compiles_to_a_back_edge_jump_and_a_must_be_boolean_fallback() {
        let code = compile_foo("[x] whileTrue: [x]");
        let back_edge =
            code.instructions.iter().enumerate().find_map(
                |(index, instruction)| match instruction {
                    Instruction::Jump { target } if (*target as usize) < index => Some(*target),
                    _ => None,
                },
            );
        assert!(
            back_edge.is_some(),
            "expected a backward Jump, got {:#?}",
            code.instructions
        );
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::SendPublic { selector, .. } if selector.as_str() == "mustBeBoolean"
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

    #[test]
    fn a_real_block_nested_in_an_inlined_block_counts_one_hop_fewer() {
        // `[ x ]` sits two resolver scopes below the method (inner block ->
        // inlined block -> method) but only one frame below it at run time.
        let code = compile_foo("x ifTrue: [ [ x ] value ] ifFalse: [nil]");
        let nested = code
            .instructions
            .iter()
            .find_map(|instruction| match instruction {
                Instruction::MakeClosure { code, .. } => Some(code.clone()),
                _ => None,
            });
        let nested = nested.expect("the true branch creates a closure");
        assert_eq!(
            nested.instructions[0],
            Instruction::LoadOuterLocal {
                destination: 0,
                depth: 1,
                index: 0
            }
        );
    }

    #[test]
    fn a_return_inside_an_inlined_block_follows_the_enclosing_code_kind() {
        let code = compile_foo("x ifTrue: [^1] ifFalse: [^2]");
        assert_eq!(
            code.instructions
                .iter()
                .filter(|instruction| matches!(instruction, Instruction::Return { .. }))
                .count(),
            3,
            "{:#?}",
            code.instructions
        );
        assert!(!has(&code, |instruction| matches!(
            instruction,
            Instruction::NonLocalReturn { .. }
        )));
        // Inside a block body the same `^` is a non-local return.
        let code = compile_foo("[ x ifTrue: [^1] ifFalse: [^2] ]");
        let Some(Instruction::MakeClosure { code: block, .. }) = code.instructions.first() else {
            panic!()
        };
        assert!(block
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::NonLocalReturn { .. })));
    }

    #[test]
    fn inlined_statements_do_not_clobber_live_registers_of_the_enclosing_statement() {
        // `x foo: (x bar) with: (x ifTrue: [x baz. 1] ifFalse: [2])`: the
        // first argument's register must survive the inlined `x baz` statement.
        let code = compile_foo("x foo: (x bar) with: (x ifTrue: [x baz. 1] ifFalse: [2])");
        let bar_destination = code
            .instructions
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
            .instructions
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
        let Some(Instruction::SendPublic {
            selector, argument_registers, ..
        }) = code.instructions.iter().rev().find(
            |instruction| matches!(instruction, Instruction::SendPublic { selector, .. } if selector.as_str() == "foo:with:"),
        )
        else {
            panic!("expected a foo:with: send")
        };
        assert_eq!(selector.as_str(), "foo:with:");
        assert_eq!(argument_registers[0], bar_destination);
    }

    #[test]
    fn and_or_coerce_the_block_value_and_short_circuit() {
        let code = compile_foo("x and: [x]");
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::LoadLiteral {
                literal: BytecodeLiteral::Boolean(true),
                ..
            }
        )));
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::LoadLiteral {
                literal: BytecodeLiteral::Boolean(false),
                ..
            }
        )));
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
        assert_eq!(code.register_count, 7, "{:#?}", code.instructions);
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
            0..code.instructions.len()
        );
    }
}
