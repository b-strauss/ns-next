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
use crate::bytecode::{CallSite, CodeKind, CompiledCode, Instruction, Literal as BytecodeLiteral};
use crate::mixin::Mixin;
use crate::types::Binding;
use crate::Symbol;

type Bindings = Rc<HashMap<Span, Binding>>;

pub(crate) fn compile_method(method: &ast::MethodDeclaration, bindings: &Bindings) -> CompiledCode {
    let mut body = Body::new(
        CodeKind::Method,
        super::argument_count_of(&method.pattern),
        u16::try_from(method.body.temporaries.len())
            .expect("internal: more than u16::MAX temporaries in one method"),
        bindings,
    );
    body.statements_and_end(&method.body.statements);
    body.finish(method.span)
}

pub(crate) fn compile_mixin_bodies(mixin: &mut Mixin, bindings: &Bindings) {
    mixin.initializer_code = Some(Rc::new(initializer(mixin, bindings)));
    mixin.superclass_clause_code = superclass_clause(mixin, bindings).map(Rc::new);
    mixin.superclass_message_code = superclass_message(mixin, bindings).map(Rc::new);
}

/// Each own slot's initializer followed by `InitializeSlot`, one statement
/// apiece, then the `initExprs`, then `self`. `InitializeSlot`'s index is the
/// slot's position among the mixin's own slots, never an absolute field index
/// (`docs/design/compiler.md`, "One body per mixin").
fn initializer(mixin: &Mixin, bindings: &Bindings) -> CompiledCode {
    let mut body = Body::new(
        CodeKind::Initializer,
        super::factory_argument_count(mixin),
        0,
        bindings,
    );
    let mut source_span: Option<Span> = None;
    for (own_index, slot) in mixin.slots.iter().enumerate() {
        let Some(declaration) = &slot.declaration else {
            continue;
        };
        let Some(initializer) = &declaration.initializer else {
            continue;
        };
        let start = body.instructions.len();
        let source = body.value(initializer);
        body.emit(Instruction::InitializeSlot {
            source,
            own_index: own_index as u32,
        });
        body.record_statement(start);
        source_span = Some(super::merge_span(source_span, declaration.span));
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

/// The superclass clause as a single `^ec`. The expression is
/// `interpreter::nested`'s, the one definition of what a clause evaluates.
fn superclass_clause(mixin: &Mixin, bindings: &Bindings) -> Option<CompiledCode> {
    let clause = mixin.declaration.as_ref()?.superclass.as_ref()?;
    let statements = [Statement::Return(
        crate::interpreter::nested::superclass_clause_expression(clause),
    )];
    let mut body = Body::new(CodeKind::SuperclassClause, 0, 0, bindings);
    body.statements_and_end(&statements);
    Some(body.finish(clause.span))
}

/// The clause's message arguments as a single `^{argument. argument}`, run with
/// the declaring class's own factory arguments.
fn superclass_message(mixin: &Mixin, bindings: &Bindings) -> Option<CompiledCode> {
    let clause = mixin.declaration.as_ref()?.superclass.as_ref()?;
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
    argument_count: u16,
    /// The first register above the named locals. Every register from here
    /// on is written by exactly one instruction.
    first_scratch: u16,
    next_register: u16,
    instructions: Vec<Instruction>,
    statement_program_counter: Vec<(usize, Range<usize>)>,
}

impl<'a> Body<'a> {
    fn new(
        kind: CodeKind,
        argument_count: u16,
        temporary_count: u16,
        bindings: &'a Bindings,
    ) -> Self {
        // Checked in every build, like `fresh`: a wrapped sum would place
        // scratch registers on top of the named locals.
        let first_scratch = argument_count
            .checked_add(temporary_count)
            .expect("internal: more than u16::MAX named locals in one body");
        Body {
            bindings,
            kind,
            argument_count,
            first_scratch,
            next_register: first_scratch,
            instructions: Vec::new(),
            statement_program_counter: Vec::new(),
        }
    }

    /// A register nothing has written yet. Checked in every build rather than
    /// only under `debug_assertions`: a register index that wrapped would be
    /// silently wrong code, and compiling is not a hot path.
    fn fresh(&mut self) -> u16 {
        let register = self.next_register;
        self.next_register = register
            .checked_add(1)
            .expect("internal: more than u16::MAX registers in one body");
        register
    }

    fn emit(&mut self, instruction: Instruction) {
        self.instructions.push(instruction);
    }

    fn binding(&self, span: Span) -> Option<Binding> {
        self.bindings.get(&span).copied()
    }

    fn record_statement(&mut self, start: usize) {
        let index = self.statement_program_counter.len();
        self.statement_program_counter
            .push((index, start..self.instructions.len()));
    }

    fn statements_and_end(&mut self, statements: &[Statement]) {
        let mut last_value = None;
        for statement in statements {
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
        let start = self.instructions.len();
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
            // No binding: a bare builtin class name, or the deferred
            // unresolved-identifier error (`docs/design/compiler.md`,
            // "Compilation cannot fail"). A bare identifier bound `SuperSend`
            // is a `super`-prefixed superclass-clause receiver, which falls to
            // the same case.
            Some(Binding::SuperSend) | None => {
                let destination = self.fresh();
                match crate::interpreter::instantiate::builtin_kind_from_name(&id.name) {
                    Some(kind) => self.emit(Instruction::LoadBuiltinClass { destination, kind }),
                    None => self.emit(Instruction::LoadLiteral {
                        destination,
                        literal: BytecodeLiteral::UnresolvedIdentifier(id.name.clone()),
                    }),
                }
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
            Some(Binding::SuperSend) | None => panic!(
                "internal: unresolved assignment target `{}` (the resolver should have rejected this)",
                target.name
            ),
        }
        value_register
    }

    fn send(&mut self, send: &ast::Send) -> u16 {
        let selector = selector_of(&send.message);
        match self.binding(send.span) {
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
            // (`docs/design/compiler.md`, "How the limit counts").
            Some(Binding::Local(_) | Binding::OuterLocal { .. }) | None => {
                let receiver_register = self.value(&send.receiver);
                if control_flow::recognize(send).is_some() {
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
            u16::try_from(parameters.len())
                .expect("internal: more than u16::MAX parameters in one block"),
            u16::try_from(block.body.temporaries.len())
                .expect("internal: more than u16::MAX temporaries in one block"),
            self.bindings,
        );
        inner.statements_and_end(&block.body.statements);
        let code = inner.finish(block.span);
        let destination = self.fresh();
        self.emit(Instruction::MakeClosure {
            destination,
            parameters: Rc::new(parameters),
            code: Rc::new(code),
        });
        destination
    }

    /// The reference lowering is the uncached oracle (`docs/design/dispatch.md`,
    /// "The reference lowering"): every send it emits carries `UNCACHED`, so a
    /// send it compiles always runs the full lookup, never a cache hit.
    fn emit_send(&mut self, build: impl FnOnce(u16, CallSite) -> Instruction) -> u16 {
        let destination = self.fresh();
        self.emit(build(destination, CallSite::UNCACHED));
        destination
    }

    fn finish(self, source: Span) -> CompiledCode {
        if cfg!(debug_assertions) {
            check_shape(&self.instructions, self.first_scratch);
        }
        CompiledCode {
            kind: self.kind,
            family_size: crate::bytecode::family_size_of(&self.instructions),
            instructions: self.instructions,
            argument_count: self.argument_count,
            register_count: self.next_register,
            statement_program_counter: self.statement_program_counter,
            source,
        }
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
        | Instruction::InitializeSlot { source, .. } => vec![*source],
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
        Instruction::LoadLiteral { .. }
        | Instruction::LoadSelf { .. }
        | Instruction::LoadBuiltinClass { .. }
        | Instruction::LoadOuterLocal { .. }
        | Instruction::MakeClosure { .. }
        | Instruction::Jump { .. } => vec![],
    }
}

/// The register `instruction` writes, if any. Exhaustive on purpose: a new
/// instruction has to decide here whether it writes a register.
fn destination(instruction: &Instruction) -> Option<u16> {
    match instruction {
        Instruction::LoadLiteral { destination, .. }
        | Instruction::LoadSelf { destination }
        | Instruction::LoadBuiltinClass { destination, .. }
        | Instruction::Move { destination, .. }
        | Instruction::LoadOuterLocal { destination, .. }
        | Instruction::SendLexical { destination, .. }
        | Instruction::SendPublic { destination, .. }
        | Instruction::SendInlinable { destination, .. }
        | Instruction::SendSuper { destination, .. }
        | Instruction::MakeClosure { destination, .. }
        | Instruction::MakeArray { destination, .. } => Some(*destination),
        Instruction::StoreLocal { .. }
        | Instruction::StoreOuterLocal { .. }
        | Instruction::InitializeSlot { .. }
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
            Newspeak3
            'T'
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
            code.instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::SendInlinable { selector, .. } if selector.as_str() == "ifTrue:")),
            "{code:?}"
        );
        assert!(!code
            .instructions
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
                    .instructions
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
                code.instructions.iter().any(
                    |instruction| matches!(instruction, Instruction::SendInlinable { selector: inlined_selector, .. } if inlined_selector.as_str() == selector)
                ),
                "{declaration}: {code:?}"
            );
        }
    }

    fn closure_code(instruction: &Instruction) -> &CompiledCode {
        let Instruction::MakeClosure { code, .. } = instruction else {
            panic!("expected a MakeClosure, got {instruction:?}");
        };
        code
    }

    fn no_branch(code: &CompiledCode) -> bool {
        code.instructions.iter().all(|instruction| {
            !matches!(
                instruction,
                Instruction::Jump { .. }
                    | Instruction::JumpIfNotBoolean { .. }
                    | Instruction::BranchIfFalse { .. }
                    | Instruction::BranchIfTrue { .. }
            )
        })
    }

    #[test]
    fn reading_a_local_copies_it_into_a_fresh_register() {
        let code = compile(&method("foo: x = ( ^x )"), 0);
        assert_eq!(
            code.instructions,
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
        assert_eq!(code.instructions.len(), 5);
        assert_eq!(
            code.instructions[0],
            Instruction::Move {
                destination: 1,
                source: 0
            }
        );
        assert!(matches!(
            code.instructions[1],
            Instruction::MakeClosure { destination: 2, .. }
        ));
        assert!(matches!(
            code.instructions[2],
            Instruction::MakeClosure { destination: 3, .. }
        ));
        assert_eq!(
            code.instructions[3],
            inlinable("ifTrue:ifFalse:", 1, vec![2, 3], 4)
        );
        assert_eq!(code.instructions[4], Instruction::Return { register: 4 });
        assert_eq!(
            closure_code(&code.instructions[1]).instructions,
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
            &method("foo = ( | n | n := 0. [n < 3] whileTrue: [n := n + 1]. ^n )"),
            0,
        );
        assert!(no_branch(&code));
        assert_eq!(
            code.instructions[0],
            Instruction::LoadLiteral {
                destination: 1,
                literal: BytecodeLiteral::SmallInteger(0)
            }
        );
        assert_eq!(
            code.instructions[1],
            Instruction::StoreLocal { source: 1, slot: 0 }
        );
        assert!(matches!(
            code.instructions[2],
            Instruction::MakeClosure { destination: 2, .. }
        ));
        assert!(matches!(
            code.instructions[3],
            Instruction::MakeClosure { destination: 3, .. }
        ));
        assert_eq!(code.instructions[4], inlinable("whileTrue:", 2, vec![3], 4));
        assert_eq!(
            code.instructions[5],
            Instruction::Move {
                destination: 5,
                source: 0
            }
        );
        assert_eq!(code.instructions[6], Instruction::Return { register: 5 });
        assert_eq!(
            code.statement_program_counter,
            vec![(0, 0..2), (1, 2..5), (2, 5..7)]
        );
        assert_eq!(
            closure_code(&code.instructions[2]).instructions,
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
            closure_code(&code.instructions[3]).instructions,
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
        let block = closure_code(&code.instructions[1]);
        assert_eq!(block.kind, CodeKind::Block);
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
    fn an_assignments_value_is_its_fresh_register_never_the_local() {
        let code = compile(&method("foo = ( | t | ^t := 3 )"), 0);
        assert_eq!(
            code.instructions,
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
            code.instructions,
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
            closure_code(&code.instructions[0]).instructions,
            vec![
                Instruction::LoadLiteral {
                    destination: 0,
                    literal: BytecodeLiteral::Nil
                },
                Instruction::ImplicitReturn { register: 0 },
            ]
        );
    }
}
