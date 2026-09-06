//! Bytecode compiler entry points: `compile_method`, `compile_initializer`,
//! `compile_module`. See
//! `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md`.
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
use crate::types::{Binding, ClassRef, ObjectData};
use crate::Symbol;

use lower::Lowerer;

/// `instantiate::pattern_param_count` narrowed to the `u16` `CompiledCode`
/// stores an argument count in. A pattern's parameter count is bounded by
/// the keyword parts a parser can produce for one message, so the cast can
/// never truncate in practice; the count itself has exactly one definition,
/// shared with the interpreter's factory-arity guard.
fn num_args_of(pattern: &ast::MessagePattern) -> u16 {
    crate::interpreter::instantiate::pattern_param_count(pattern) as u16
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

/// Compiles a loaded class's own instance-initializer body: each slot with
/// an initializer (declaration order) as `<expr>; InitSlot`, then each
/// `initExpr` (a top-level `^` is evaluated and discarded), then
/// `LoadSelf; ImplicitReturn` -- the spec's "Class instantiation" section.
/// Delegates to `compile_mixin_initializer` with the class's own, already
/// applied `slot_index` -- see that function's doc comment for why a mixin
/// with no application yet (every nested mixin, until the accessor
/// materializes one) takes a different `slot_index` instead.
pub fn compile_initializer(
    class: &ClassRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> CompiledCode {
    let (mixin, slot_index) = {
        let borrowed = class.borrow();
        let ObjectData::Class(data) = &borrowed.data else {
            panic!("internal: compile_initializer called on a non-class object");
        };
        (
            data.application.mixin.clone(),
            data.application.slot_index.clone(),
        )
    };
    compile_mixin_initializer(&mixin, &slot_index, bindings)
}

/// The shared body behind `compile_initializer`: `mixin`'s own slots
/// (against `slot_index`'s absolute offsets) then its own `init_exprs`.
/// `num_args` is `mixin`'s own factory parameter count -- the resolver
/// scoped this code with exactly those parameters as `Binding::Local`.
///
/// `slot_index` is the caller's answer for where each of `mixin`'s own
/// slots (and anything it inherits) lands in the instance layout. For the
/// root class that is the real, already-applied `MixinApplication`'s
/// layout (`compile_initializer` above). A nested mixin or a class side has
/// no application at compile time -- `build_mixin` leaves every nested
/// declaration unapplied, since applying one means resolving a superclass
/// and only the accessor (materializing a class object per enclosing
/// instance) ever does that -- so `compile_module`'s tree walk below passes
/// a *structural* `slot_index` instead: the mixin's own slots at their
/// declaration-order offset, with nothing inherited. That is not an
/// approximation in this milestone: `resolve_superclass_receiver` only ever
/// resolves a builtin superclass, and no builtin declares a slot, so "zero
/// inherited slots" is the actual answer for every mixin this compiler can
/// see today, nested or not. A later milestone that lets a nested class's
/// superclass carry slots must revisit this alongside teaching the accessor
/// to resolve one.
fn compile_mixin_initializer(
    mixin: &Rc<Mixin>,
    slot_index: &HashMap<Symbol, usize>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> CompiledCode {
    let num_args = mixin.factory.as_deref().map(num_args_of).unwrap_or(0);
    let mut lowerer = Lowerer::new(CodeKind::Initializer, num_args, 0, bindings, vec![]);
    let mut source: Option<Span> = None;
    for slot in &mixin.instance_slots {
        if let Some(init) = &slot.init {
            let field_index = *slot_index
                .get(&Symbol::intern(&slot.name.name))
                .expect("internal: apply_mixin (or its structural stand-in) assigns every own slot an absolute index");
            lowerer.lower_slot_init(init, field_index as u32);
            source = Some(merge_span(source, slot.span));
        }
    }
    for stmt in &mixin.init_exprs {
        lowerer.lower_statement(stmt);
        source = Some(merge_span(source, stmt_span(stmt)));
    }
    lowerer.finish_body(&mixin.init_exprs);
    lowerer.finish(source.unwrap_or(Span::new(0, 0)))
}

/// A mixin's own slots at their declaration-order offset, with nothing
/// inherited -- `compile_mixin_tree`'s stand-in `slot_index` for every
/// mixin that has no real `MixinApplication` (nested classes and class
/// sides; see `compile_mixin_initializer`'s doc comment for why this is
/// exact, not approximate, in this milestone).
fn structural_slot_index(mixin: &Mixin) -> HashMap<Symbol, usize> {
    mixin
        .instance_slots
        .iter()
        .enumerate()
        .map(|(i, slot)| (Symbol::intern(&slot.name.name), i))
        .collect()
}

/// Compiles every method and the initializer of `mixin`, then recurses into
/// its nested declarations and its class side -- the whole mixin tree, not
/// only the root. Both maps are keyed by mixin/method identity
/// (`mixin_key`/`method_key`), never by class object: `newspeak-spec.pdf`
/// §6.2 has every application of a mixin share its code, but §6 gives every
/// enclosing instance of a nested class its own class object, so a
/// class-object key would compile the same body again for every instance --
/// exactly what walking the tree once here, ahead of any instantiation, is
/// meant to avoid.
fn compile_mixin_tree(
    mixin: &Rc<Mixin>,
    slot_index: &HashMap<Symbol, usize>,
    module: &mut CompiledModule,
    bindings: &Rc<HashMap<Span, Binding>>,
) {
    for method in mixin.methods.values() {
        let compiled = compile_method(method, bindings);
        module.methods.insert(method_key(method), Rc::new(compiled));
    }
    let compiled_init = compile_mixin_initializer(mixin, slot_index, bindings);
    module
        .initializers
        .insert(mixin_key(mixin), Rc::new(compiled_init));
    for nested in &mixin.nested {
        compile_mixin_tree(nested, &structural_slot_index(nested), module, bindings);
    }
    if let Some(class_side) = &mixin.class_side {
        compile_mixin_tree(
            class_side,
            &structural_slot_index(class_side),
            module,
            bindings,
        );
    }
}

pub fn compile_module(
    root_class: &ClassRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> CompiledModule {
    let (root_mixin, root_slot_index) = {
        let borrowed = root_class.borrow();
        match &borrowed.data {
            ObjectData::Class(data) => (
                data.application.mixin.clone(),
                data.application.slot_index.clone(),
            ),
            _ => panic!("internal: compile_module called on a non-class object"),
        }
    };
    let mut module = CompiledModule {
        methods: HashMap::new(),
        initializers: HashMap::new(),
        bindings: bindings.clone(),
    };
    compile_mixin_tree(&root_mixin, &root_slot_index, &mut module, bindings);
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
