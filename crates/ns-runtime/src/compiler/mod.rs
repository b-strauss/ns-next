//! Bytecode compiler entry points: `compile_method`, `compile_initializer`,
//! `compile_module`. See
//! `docs/design/compiler.md`.
//! Compiling an already-resolved module is a total, deterministic lowering
//! that can never raise a `NewspeakError`; a failure here is an `ns-runtime`
//! bug and panics.

pub(crate) mod control_flow;
pub(crate) mod lower;

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast::{self, Expr, Stmt};
use ns_syntax::Span;

use crate::bytecode::{method_key, CodeKind, CompiledCode, CompiledModule};
use crate::mixin::{mixin_key, Mixin};
use crate::types::{Binding, ClassRef};

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
fn factory_num_args(mixin: &Rc<Mixin>) -> u16 {
    // Same reasoning as `num_args_of` above: a factory's parameter count is
    // the number of keyword parts one message pattern has, which no parser
    // and no image (which stores it in a `u16` field) can push past `u16`.
    mixin.factory.map_or(0, |factory| factory.num_args as u16)
}

pub fn compile_method(
    method: &ast::MethodDecl,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> CompiledCode {
    let num_args = num_args_of(&method.pattern);
    let num_named_temps = method.body.temps.len() as u16;
    let mut lowerer = Lowerer::new(
        CodeKind::Method,
        num_args,
        num_named_temps,
        bindings,
        vec![],
    );
    for stmt in &method.body.stmts {
        lowerer.lower_statement(stmt);
    }
    lowerer.finish_body(&method.body.stmts);
    lowerer.finish(method.span)
}

/// Compiles a loaded class's own instance-initializer body. Delegates to
/// `compile_mixin_initializer`; kept as a `ClassRef`-taking entry point
/// because `ns-cli`'s `disasm` and this module's own tests address a class
/// directly, without going through `compile_module`'s tree walk.
///
/// Takes `heap` rather than building one. `Heap` owns real per-instance
/// state -- both the frame and the object arenas -- so a freshly built
/// `Heap` is not the interchangeable, counter-only value it used to be:
/// `class` is an index into a specific `Heap.objects`, and reading it
/// through any other `Heap` either indexes out of range and panics, or
/// silently names a different object entirely.
pub fn compile_initializer(
    heap: &crate::Heap,
    class: &ClassRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> CompiledCode {
    let mixin = match heap.class_application(class) {
        Some(app) => app.mixin.clone(),
        None => panic!("internal: compile_initializer called on a non-class object"),
    };
    compile_mixin_initializer(&mixin, bindings)
}

/// The shared body behind `compile_initializer` and `compile_module`'s tree
/// walk: each of `mixin`'s own slots with an initializer (declaration
/// order) as `<expr>; InitSlot`, then each `initExpr` (a top-level `^` is
/// evaluated and discarded), then `LoadSelf; ImplicitReturn` -- the spec's
/// "Class instantiation" section. `num_args` is `mixin`'s own factory
/// parameter count -- the resolver scoped this code with exactly those
/// parameters as `Binding::Local`.
///
/// Needs no `slot_index` (unlike before the review's round-1 fix): each
/// `InitSlot` now carries `own_index`, the slot's position among `mixin`'s
/// *own* slots only (plain declaration-order `enumerate()`, independent of
/// any superclass) -- never an absolute field index, because this one
/// compiled body runs for every application of `mixin` (`compile_module`
/// keys by `mixin_key`, not by class object, per §6.2), and different
/// applications can legitimately disagree about where the mixin's own
/// slots start. `vm::run_code` adds the executing frame's `slot_base` --
/// set per level, at run time, from that level's own real
/// `MixinApplication.own_slot_base` -- to turn `own_index` into the
/// absolute index the instance array actually needs.
fn compile_mixin_initializer(
    mixin: &Rc<Mixin>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> CompiledCode {
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
        lowerer.lower_statement(stmt);
        source = Some(merge_span(source, stmt_span(stmt)));
    }
    lowerer.finish_body(&mixin.init_exprs);
    lowerer.finish(source.unwrap_or(Span::new(0, 0)))
}

/// A mixin's superclass clause as executable code: the expression
/// `newspeak-spec.pdf` §6.1 calls `ec`, lowered as a single synthetic `^ec`
/// so the body ends in the `Return` that hands the class object back.
///
/// The expression itself comes from `interpreter::nested`'s
/// `superclass_clause_expr`, the *one* definition of what a superclass
/// clause evaluates -- the tree-walker evaluates that same `ast::Expr`
/// directly. Building a second, compiler-local notion of "the clause's
/// expression" here is exactly how the two engines would come to disagree
/// about a clause that is anything more interesting than a builtin class
/// name.
///
/// Returns `None` for a mixin with no clause (a class side, a builtin, or
/// `class C = (...)`), which the accessor answers with `Object` under both
/// engines without running any code at all.
fn compile_superclass_clause(
    mixin: &Rc<Mixin>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> Option<CompiledCode> {
    let clause = mixin.decl.as_ref()?.superclass.as_ref()?;
    let stmts = vec![Stmt::Return(
        crate::interpreter::nested::superclass_clause_expr(clause),
    )];
    let mut lowerer = Lowerer::new(CodeKind::SuperclassClause, 0, 0, bindings, vec![]);
    for stmt in &stmts {
        lowerer.lower_statement(stmt);
    }
    lowerer.finish_body(&stmts);
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
/// clause's arguments are -- the tree-walker evaluates that same
/// `ast::Expr`. `None` when the clause has no message, or a message with no
/// arguments: then there is nothing to evaluate under either engine.
fn compile_superclass_message(
    mixin: &Rc<Mixin>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> Option<CompiledCode> {
    let clause = mixin.decl.as_ref()?.superclass.as_ref()?;
    let args = crate::interpreter::nested::superclass_clause_message_args_expr(clause)?;
    let num_args = factory_num_args(mixin);
    let stmts = vec![Stmt::Return(args)];
    let mut lowerer = Lowerer::new(CodeKind::SuperclassMessage, num_args, 0, bindings, vec![]);
    for stmt in &stmts {
        lowerer.lower_statement(stmt);
    }
    lowerer.finish_body(&stmts);
    Some(lowerer.finish(clause.span))
}

/// Takes `heap` rather than building one; see `compile_initializer`'s doc
/// comment for why a throwaway `Heap` is unsafe.
pub fn compile_module(
    heap: &crate::Heap,
    root_class: &ClassRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> CompiledModule {
    let root_mixin = match heap.class_application(root_class) {
        Some(app) => app.mixin.clone(),
        None => panic!("internal: compile_module called on a non-class object"),
    };
    let mut module = CompiledModule {
        methods: HashMap::new(),
        initializers: HashMap::new(),
        superclass_clauses: HashMap::new(),
        superclass_messages: HashMap::new(),
        bindings: bindings.clone(),
    };
    // `crate::mixin::preorder` is also what `image::encode`/`decode` walk
    // the tree in, for the same code and the same reason: the two must
    // enumerate the same mixins in the same order (see its doc comment).
    let mut order = Vec::new();
    crate::mixin::preorder(&root_mixin, &mut order);
    for mixin in &order {
        for method in mixin.methods.values() {
            // Compiling happens from source, so every entry here has its
            // declaration; a mixin whose methods have none came from an
            // image and already carries their compiled bodies.
            let Some(decl) = &method.decl else { continue };
            let compiled = compile_method(decl, bindings);
            module.methods.insert(method_key(method), Rc::new(compiled));
        }
        let compiled_init = compile_mixin_initializer(mixin, bindings);
        module
            .initializers
            .insert(mixin_key(mixin), Rc::new(compiled_init));
        if let Some(clause) = compile_superclass_clause(mixin, bindings) {
            module
                .superclass_clauses
                .insert(mixin_key(mixin), Rc::new(clause));
        }
        if let Some(message) = compile_superclass_message(mixin, bindings) {
            module
                .superclass_messages
                .insert(mixin_key(mixin), Rc::new(message));
        }
    }
    module
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
