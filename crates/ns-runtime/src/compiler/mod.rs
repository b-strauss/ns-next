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

use crate::bytecode::{class_key, method_key, CodeKind, CompiledCode, CompiledModule};
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
/// `num_args` is the class's own factory parameter count: the resolver
/// scoped this code with exactly those parameters as `Binding::Local`.
pub fn compile_initializer(
    class: &ClassRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> CompiledCode {
    let (slots, slot_index, init_exprs, num_args) = {
        let borrowed = class.borrow();
        let ObjectData::Class(data) = &borrowed.data else {
            panic!("internal: compile_initializer called on a non-class object");
        };
        (
            data.instance_slots.clone(),
            data.slot_index.clone(),
            data.init_exprs.clone(),
            data.factory.as_deref().map(num_args_of).unwrap_or(0),
        )
    };
    let mut lowerer = Lowerer::new(CodeKind::Initializer, num_args, 0, bindings, vec![]);
    let mut source: Option<Span> = None;
    for slot in &slots {
        if let Some(init) = &slot.init {
            let field_index = *slot_index
                .get(&Symbol::intern(&slot.name.name))
                .expect("internal: load_class assigns every own slot an absolute index");
            lowerer.lower_slot_init(init, field_index as u32);
            source = Some(merge_span(source, slot.span));
        }
    }
    for stmt in &init_exprs {
        lowerer.lower_statement(stmt);
        source = Some(merge_span(source, stmt_span(stmt)));
    }
    lowerer.finish_body(&init_exprs);
    lowerer.finish(source.unwrap_or(Span::new(0, 0)))
}

pub fn compile_module(
    root_class: &ClassRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> CompiledModule {
    let methods: Vec<Rc<ast::MethodDecl>> = {
        let borrowed = root_class.borrow();
        match &borrowed.data {
            ObjectData::Class(data) => data.methods.values().cloned().collect(),
            _ => Vec::new(),
        }
    };
    let mut module = CompiledModule {
        methods: HashMap::new(),
        initializers: HashMap::new(),
        bindings: bindings.clone(),
    };
    for method in methods {
        let compiled = compile_method(&method, bindings);
        module
            .methods
            .insert(method_key(&method), Rc::new(compiled));
    }
    module.initializers.insert(
        class_key(root_class),
        Rc::new(compile_initializer(root_class, bindings)),
    );
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
