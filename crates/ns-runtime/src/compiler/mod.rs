//! Bytecode compiler entry points, and the choice between the optimized
//! lowering (`lower.rs`) and the reference lowering it is checked against
//! (`reference.rs`).
//! Compiling an already-resolved module is a total, deterministic lowering
//! that can never raise a `NewspeakError`; a failure here is an `ns-runtime`
//! bug and panics.

pub(crate) mod control_flow;
pub(crate) mod lower;
pub(crate) mod reference;

/// Which lowering compiles a module: the optimized one every program runs
/// under, or the deliberately naive reference lowering it is checked against
/// (`docs/design/compiler.md`, "The reference lowering"). Chosen once per
/// `Interpreter`, so one interpreter never mixes the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lowering {
    Optimized,
    Reference,
}

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast::{self, Expression, Statement};
use ns_syntax::Span;

use crate::bytecode::{CodeKind, CompiledCode};
use crate::mixin::Mixin;
use crate::types::Binding;

use lower::Lowerer;

/// `instantiate::pattern_parameter_count` narrowed to the `u16` `CompiledCode`
/// stores an argument count in. A pattern's parameter count is bounded by
/// the keyword parts a parser can produce for one message, so the cast can
/// never truncate in practice; the count itself has exactly one definition,
/// shared with the interpreter's factory-arity guard.
fn argument_count_of(pattern: &ast::MessagePattern) -> u16 {
    crate::interpreter::instantiate::pattern_parameter_count(pattern) as u16
}

/// A mixin's primary factory parameter count, narrowed to the `u16` a
/// `CompiledCode` stores its arity in. `0` for a mixin with no factory of
/// its own -- a builtin's, a class side's.
fn factory_argument_count(mixin: &Mixin) -> u16 {
    // Same reasoning as `argument_count_of` above: a factory's parameter count
    // is the number of keyword parts one message pattern has, which no parser
    // and no image (which stores it in a `u16` field) can push past `u16`.
    mixin
        .factory
        .map_or(0, |factory| factory.argument_count as u16)
}

pub fn compile_method(
    method: &ast::MethodDeclaration,
    bindings: &Rc<HashMap<Span, Binding>>,
    lowering: Lowering,
) -> CompiledCode {
    if lowering == Lowering::Reference {
        return reference::compile_method(method, bindings);
    }
    let argument_count = argument_count_of(&method.pattern);
    let named_temporary_count = method.body.temporaries.len() as u16;
    let mut lowerer = Lowerer::new(
        CodeKind::Method,
        argument_count,
        named_temporary_count,
        bindings,
        vec![],
        0,
    );
    lowerer.lower_body(&method.body.statements);
    lowerer.finish(method.span)
}

/// Compiles `mixin`'s per-mixin bodies onto it: the instance initializer
/// always, the superclass clause and its message's arguments when the
/// declaration has them. Called while the mixin is still a value, before it is
/// wrapped in the `Rc` every application shares -- the one moment it can be
/// written.
pub(crate) fn compile_mixin_bodies(
    mixin: &mut Mixin,
    bindings: &Rc<HashMap<Span, Binding>>,
    lowering: Lowering,
) {
    if lowering == Lowering::Reference {
        reference::compile_mixin_bodies(mixin, bindings);
        return;
    }
    mixin.initializer_code = Some(Rc::new(compile_mixin_initializer(mixin, bindings)));
    mixin.superclass_clause_code = compile_superclass_clause(mixin, bindings).map(Rc::new);
    mixin.superclass_message_code = compile_superclass_message(mixin, bindings).map(Rc::new);
}

/// A mixin's instance initializer: each of `mixin`'s own slots with an
/// initializer (declaration order) as `<expression>; InitializeSlot`, then each
/// `initExpr` (a top-level `^` is evaluated and discarded), then
/// `LoadSelf; ImplicitReturn` -- `docs/design/object-model.md`, "Instantiation".
/// `argument_count` is `mixin`'s own factory parameter count -- the resolver
/// scoped this code with exactly those parameters as `Binding::Local`.
///
/// Needs no absolute field index: each `InitializeSlot` carries `own_index`, the slot's
/// position among `mixin`'s *own* slots only (plain declaration-order
/// `enumerate()`, independent of any superclass) -- never an absolute field
/// index, because this one compiled body runs for every application of
/// `mixin` (it lives on the mixin, not on a class object, per §6.2), and
/// different applications can legitimately disagree about where the mixin's
/// own slots start. The VM adds the executing frame's `slot_base` --
/// set per level, at run time, from that level's own real
/// `MixinApplication.own_slot_base` -- to turn `own_index` into the
/// absolute index the instance array actually needs.
fn compile_mixin_initializer(mixin: &Mixin, bindings: &Rc<HashMap<Span, Binding>>) -> CompiledCode {
    let argument_count = factory_argument_count(mixin);
    let mut lowerer = Lowerer::new(
        CodeKind::Initializer,
        argument_count,
        0,
        bindings,
        vec![],
        0,
    );
    let mut source: Option<Span> = None;
    for (own_index, slot) in mixin.slots.iter().enumerate() {
        let Some(declaration) = &slot.declaration else {
            continue;
        };
        if let Some(initializer) = &declaration.initializer {
            lowerer.lower_slot_initializer(initializer, own_index as u32);
            source = Some(merge_span(source, declaration.span));
        }
    }
    for statement in &mixin.initializer_expressions {
        source = Some(merge_span(source, statement_span(statement)));
    }
    lowerer.lower_body(&mixin.initializer_expressions);
    lowerer.finish(source.unwrap_or(Span::new(0, 0)))
}

/// A mixin's superclass clause as executable code: the expression
/// `newspeak-spec.pdf` §6.1 calls `ec`, lowered as a single synthetic `^ec`
/// so the body ends in the `Return` that hands the class object back.
///
/// The expression itself comes from `interpreter::nested`'s
/// `superclass_clause_expression`, the *one* definition of what a superclass
/// clause evaluates.
///
/// Returns `None` for a mixin with no clause (a class side, a builtin, or
/// `class C = (...)`), which the accessor answers with `Object` without
/// running any code at all.
fn compile_superclass_clause(
    mixin: &Mixin,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> Option<CompiledCode> {
    let clause = mixin.declaration.as_ref()?.superclass.as_ref()?;
    let statements = vec![Statement::Return(
        crate::interpreter::nested::superclass_clause_expression(clause),
    )];
    let mut lowerer = Lowerer::new(CodeKind::SuperclassClause, 0, 0, bindings, vec![], 0);
    lowerer.lower_body(&statements);
    Some(lowerer.finish(clause.span))
}

/// A mixin's superclass-clause *message arguments* as executable code: the
/// arguments of `newspeak-spec.pdf` §6.1's deferred `message`, lowered as a
/// single synthetic `^{argument. argument}` so the body ends in the `Return`
/// that hands the whole argument list back at once.
///
/// `argument_count` is this mixin's own factory parameter count, not zero as in
/// `compile_superclass_clause`: §6.2 evaluates the message in the
/// instantiation's own initializer context, where the resolver scoped these
/// expressions with exactly those parameters as `Binding::Local` (the
/// `class M usingPlatform: p = Super usingPlatform: p (...)` idiom).
///
/// The expression comes from `interpreter::nested`'s
/// `superclass_clause_message_arguments_expression`, the one definition of what
/// the clause's arguments are -- `SuperclassClause::has_arguments` is read off
/// it too. `None` when the clause has no message, or a message with no
/// arguments: then there is nothing to evaluate.
fn compile_superclass_message(
    mixin: &Mixin,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> Option<CompiledCode> {
    let clause = mixin.declaration.as_ref()?.superclass.as_ref()?;
    let arguments =
        crate::interpreter::nested::superclass_clause_message_arguments_expression(clause)?;
    let argument_count = factory_argument_count(mixin);
    let statements = vec![Statement::Return(arguments)];
    let mut lowerer = Lowerer::new(
        CodeKind::SuperclassMessage,
        argument_count,
        0,
        bindings,
        vec![],
        0,
    );
    lowerer.lower_body(&statements);
    Some(lowerer.finish(clause.span))
}

fn merge_span(accumulator: Option<Span>, next: Span) -> Span {
    match accumulator {
        Some(span) => span.merge(next),
        None => next,
    }
}

fn statement_span(statement: &Statement) -> Span {
    match statement {
        Statement::Expression(expression) | Statement::Return(expression) => {
            expression_span(expression)
        }
    }
}

fn expression_span(expression: &Expression) -> Span {
    match expression {
        Expression::Identifier(id) => id.span,
        Expression::Literal(literal) => literal_span(literal),
        Expression::Send(send) => send.span,
        Expression::Cascade(cascade) => cascade.span,
        Expression::Assign { span, .. } => *span,
        Expression::Block(block) => block.span,
        Expression::DynamicArray { span, .. } => *span,
    }
}

fn literal_span(literal: &ast::Literal) -> Span {
    match literal {
        ast::Literal::Integer { span, .. }
        | ast::Literal::Float { span, .. }
        | ast::Literal::String { span, .. }
        | ast::Literal::Symbol { span, .. }
        | ast::Literal::Character { span, .. }
        | ast::Literal::Boolean { span, .. }
        | ast::Literal::Nil { span }
        | ast::Literal::Array { span, .. }
        | ast::Literal::Pattern { span, .. } => *span,
    }
}
