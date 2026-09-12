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

use ns_syntax::ast::{self, Expr, Stmt};
use ns_syntax::Span;

use crate::bytecode::{CodeKind, CompiledCode};
use crate::mixin::Mixin;
use crate::types::Binding;

use lower::Lowerer;

/// `instantiate::pattern_param_count` narrowed to the `u16` `CompiledCode`
/// stores an argument count in. A pattern's parameter count is bounded by
/// the keyword parts a parser can produce for one message, so the cast can
/// never truncate in practice; the count itself has exactly one definition,
/// shared with the interpreter's factory-arity guard.
fn num_args_of(pattern: &ast::MessagePattern) -> u16 {
    crate::interpreter::instantiate::pattern_param_count(pattern) as u16
}

/// A mixin's primary factory parameter count, narrowed to the `u16` a
/// `CompiledCode` stores its arity in. `0` for a mixin with no factory of
/// its own -- a builtin's, a class side's.
fn factory_num_args(mixin: &Mixin) -> u16 {
    // Same reasoning as `num_args_of` above: a factory's parameter count is
    // the number of keyword parts one message pattern has, which no parser
    // and no image (which stores it in a `u16` field) can push past `u16`.
    mixin.factory.map_or(0, |factory| factory.num_args as u16)
}

pub fn compile_method(
    method: &ast::MethodDecl,
    bindings: &Rc<HashMap<Span, Binding>>,
    lowering: Lowering,
) -> CompiledCode {
    if lowering == Lowering::Reference {
        return reference::compile_method(method, bindings);
    }
    let num_args = num_args_of(&method.pattern);
    let num_named_temps = method.body.temps.len() as u16;
    let mut lowerer = Lowerer::new(
        CodeKind::Method,
        num_args,
        num_named_temps,
        bindings,
        vec![],
    );
    lowerer.lower_body(&method.body.stmts);
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
/// initializer (declaration order) as `<expr>; InitSlot`, then each
/// `initExpr` (a top-level `^` is evaluated and discarded), then
/// `LoadSelf; ImplicitReturn` -- `docs/design/object-model.md`, "Instantiation".
/// `num_args` is `mixin`'s own factory parameter count -- the resolver
/// scoped this code with exactly those parameters as `Binding::Local`.
///
/// Needs no `slot_index`: each `InitSlot` carries `own_index`, the slot's
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
    let num_args = factory_num_args(mixin);
    let mut lowerer = Lowerer::new(CodeKind::Initializer, num_args, 0, bindings, vec![]);
    let mut source: Option<Span> = None;
    for (own_index, slot) in mixin.slots.iter().enumerate() {
        let Some(decl) = &slot.decl else { continue };
        if let Some(init) = &decl.init {
            lowerer.lower_slot_init(init, own_index as u32);
            source = Some(merge_span(source, decl.span));
        }
    }
    for stmt in &mixin.init_exprs {
        source = Some(merge_span(source, stmt_span(stmt)));
    }
    lowerer.lower_body(&mixin.init_exprs);
    lowerer.finish(source.unwrap_or(Span::new(0, 0)))
}

/// A mixin's superclass clause as executable code: the expression
/// `newspeak-spec.pdf` §6.1 calls `ec`, lowered as a single synthetic `^ec`
/// so the body ends in the `Return` that hands the class object back.
///
/// The expression itself comes from `interpreter::nested`'s
/// `superclass_clause_expr`, the *one* definition of what a superclass
/// clause evaluates.
///
/// Returns `None` for a mixin with no clause (a class side, a builtin, or
/// `class C = (...)`), which the accessor answers with `Object` without
/// running any code at all.
fn compile_superclass_clause(
    mixin: &Mixin,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> Option<CompiledCode> {
    let clause = mixin.decl.as_ref()?.superclass.as_ref()?;
    let stmts = vec![Stmt::Return(
        crate::interpreter::nested::superclass_clause_expr(clause),
    )];
    let mut lowerer = Lowerer::new(CodeKind::SuperclassClause, 0, 0, bindings, vec![]);
    lowerer.lower_body(&stmts);
    Some(lowerer.finish(clause.span))
}

/// A mixin's superclass-clause *message arguments* as executable code: the
/// arguments of `newspeak-spec.pdf` §6.1's deferred `msg`, lowered as a
/// single synthetic `^{arg. arg}` so the body ends in the `Return` that
/// hands the whole argument list back at once.
///
/// `num_args` is this mixin's own factory parameter count, not zero as in
/// `compile_superclass_clause`: §6.2 evaluates the message in the
/// instantiation's own initializer context, where the resolver scoped these
/// expressions with exactly those parameters as `Binding::Local` (the
/// `class M usingPlatform: p = Super usingPlatform: p (...)` idiom).
///
/// The expression comes from `interpreter::nested`'s
/// `superclass_clause_message_args_expr`, the one definition of what the
/// clause's arguments are -- `SuperclassClause::has_args` is read off it
/// too. `None` when the clause has no message, or a message with no
/// arguments: then there is nothing to evaluate.
fn compile_superclass_message(
    mixin: &Mixin,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> Option<CompiledCode> {
    let clause = mixin.decl.as_ref()?.superclass.as_ref()?;
    let args = crate::interpreter::nested::superclass_clause_message_args_expr(clause)?;
    let num_args = factory_num_args(mixin);
    let stmts = vec![Stmt::Return(args)];
    let mut lowerer = Lowerer::new(CodeKind::SuperclassMessage, num_args, 0, bindings, vec![]);
    lowerer.lower_body(&stmts);
    Some(lowerer.finish(clause.span))
}

fn merge_span(acc: Option<Span>, next: Span) -> Span {
    match acc {
        Some(s) => s.merge(next),
        None => next,
    }
}

fn stmt_span(stmt: &Stmt) -> Span {
    match stmt {
        Stmt::Expr(e) | Stmt::Return(e) => expr_span(e),
    }
}

fn expr_span(expr: &Expr) -> Span {
    match expr {
        Expr::Ident(id) => id.span,
        Expr::Literal(lit) => literal_span(lit),
        Expr::Send(send) => send.span,
        Expr::Cascade(cascade) => cascade.span,
        Expr::Assign { span, .. } => *span,
        Expr::Block(block) => block.span,
        Expr::DynamicArray { span, .. } => *span,
    }
}

fn literal_span(lit: &ast::Literal) -> Span {
    match lit {
        ast::Literal::Int { span, .. }
        | ast::Literal::Float { span, .. }
        | ast::Literal::String { span, .. }
        | ast::Literal::Symbol { span, .. }
        | ast::Literal::Char { span, .. }
        | ast::Literal::Bool { span, .. }
        | ast::Literal::Nil { span }
        | ast::Literal::Array { span, .. }
        | ast::Literal::Pattern { span, .. } => *span,
    }
}
