//! The reference lowering: a second, deliberately naive `Expr`/`Stmt` ->
//! `Instr` lowering, which exists to be compared against `lower.rs`. Nothing
//! is inlined, every value gets a register nothing else writes, and every
//! read of a local is copied. See `docs/design/compiler.md`, "The reference
//! lowering" -- in particular why this module shares so little with
//! `lower.rs`: whatever the two share, the comparison cannot check.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;

use ns_syntax::ast::{self, Expr, Stmt};
use ns_syntax::Span;

use super::control_flow;
use super::lower::{lower_literal, selector_of};
use crate::bytecode::{CallSite, CodeKind, CompiledCode, Instr, Literal as BcLiteral};
use crate::mixin::Mixin;
use crate::types::Binding;
use crate::Symbol;

type Bindings = Rc<HashMap<Span, Binding>>;

pub(crate) fn compile_method(method: &ast::MethodDecl, bindings: &Bindings) -> CompiledCode {
    let mut body = Body::new(
        CodeKind::Method,
        super::num_args_of(&method.pattern),
        u16::try_from(method.body.temps.len())
            .expect("internal: more than u16::MAX temporaries in one method"),
        bindings,
    );
    body.statements_and_end(&method.body.stmts);
    body.finish(method.span)
}

pub(crate) fn compile_mixin_bodies(mixin: &mut Mixin, bindings: &Bindings) {
    mixin.initializer_code = Some(Rc::new(initializer(mixin, bindings)));
    mixin.superclass_clause_code = superclass_clause(mixin, bindings).map(Rc::new);
    mixin.superclass_message_code = superclass_message(mixin, bindings).map(Rc::new);
}

/// Each own slot's initializer followed by `InitSlot`, one statement apiece,
/// then the `initExprs`, then `self`. `InitSlot`'s index is the slot's
/// position among the mixin's own slots, never an absolute field index
/// (`docs/design/compiler.md`, "One body per mixin").
fn initializer(mixin: &Mixin, bindings: &Bindings) -> CompiledCode {
    let mut body = Body::new(
        CodeKind::Initializer,
        super::factory_num_args(mixin),
        0,
        bindings,
    );
    let mut source: Option<Span> = None;
    for (own_index, slot) in mixin.slots.iter().enumerate() {
        let Some(decl) = &slot.decl else { continue };
        let Some(init) = &decl.init else { continue };
        let start = body.instrs.len();
        let src = body.value(init);
        body.emit(Instr::InitSlot {
            src,
            own_index: own_index as u32,
        });
        body.record_statement(start);
        source = Some(super::merge_span(source, decl.span));
    }
    for stmt in &mixin.init_exprs {
        source = Some(super::merge_span(source, super::stmt_span(stmt)));
    }
    body.statements_and_end(&mixin.init_exprs);
    body.finish(source.unwrap_or(Span::new(0, 0)))
}

/// The superclass clause as a single `^ec`. The expression is
/// `interpreter::nested`'s, the one definition of what a clause evaluates.
fn superclass_clause(mixin: &Mixin, bindings: &Bindings) -> Option<CompiledCode> {
    let clause = mixin.decl.as_ref()?.superclass.as_ref()?;
    let stmts = [Stmt::Return(
        crate::interpreter::nested::superclass_clause_expr(clause),
    )];
    let mut body = Body::new(CodeKind::SuperclassClause, 0, 0, bindings);
    body.statements_and_end(&stmts);
    Some(body.finish(clause.span))
}

/// The clause's message arguments as a single `^{arg. arg}`, run with the
/// declaring class's own factory arguments.
fn superclass_message(mixin: &Mixin, bindings: &Bindings) -> Option<CompiledCode> {
    let clause = mixin.decl.as_ref()?.superclass.as_ref()?;
    let args = crate::interpreter::nested::superclass_clause_message_args_expr(clause)?;
    let mut body = Body::new(
        CodeKind::SuperclassMessage,
        super::factory_num_args(mixin),
        0,
        bindings,
    );
    body.statements_and_end(&[Stmt::Return(args)]);
    Some(body.finish(clause.span))
}

/// One body being lowered: a method, a block, an initializer or a
/// superclass-clause body.
struct Body<'a> {
    bindings: &'a Bindings,
    kind: CodeKind,
    num_args: u16,
    /// The first register above the named locals. Every register from here
    /// on is written by exactly one instruction.
    first_scratch: u16,
    next_register: u16,
    instrs: Vec<Instr>,
    statement_pc: Vec<(usize, Range<usize>)>,
    call_sites: Vec<CallSite>,
}

impl<'a> Body<'a> {
    fn new(kind: CodeKind, num_args: u16, num_temps: u16, bindings: &'a Bindings) -> Self {
        // Checked in every build, like `fresh`: a wrapped sum would place
        // scratch registers on top of the named locals.
        let first_scratch = num_args
            .checked_add(num_temps)
            .expect("internal: more than u16::MAX named locals in one body");
        Body {
            bindings,
            kind,
            num_args,
            first_scratch,
            next_register: first_scratch,
            instrs: Vec::new(),
            statement_pc: Vec::new(),
            call_sites: Vec::new(),
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

    fn emit(&mut self, instr: Instr) {
        self.instrs.push(instr);
    }

    fn binding(&self, span: Span) -> Option<Binding> {
        self.bindings.get(&span).copied()
    }

    fn record_statement(&mut self, start: usize) {
        let index = self.statement_pc.len();
        self.statement_pc.push((index, start..self.instrs.len()));
    }

    fn statements_and_end(&mut self, stmts: &[Stmt]) {
        let mut last_value = None;
        for stmt in stmts {
            last_value = self.statement(stmt);
        }
        self.end(matches!(stmts.last(), Some(Stmt::Return(_))), last_value);
    }

    /// One of the body's own statements, recorded in `statement_pc`. Answers
    /// the statement's value register, or `None` when it emitted a return.
    fn statement(&mut self, stmt: &Stmt) -> Option<u16> {
        let start = self.instrs.len();
        let value = match stmt {
            Stmt::Expr(expr) => Some(self.value(expr)),
            Stmt::Return(expr) => {
                let reg = self.value(expr);
                match self.kind {
                    CodeKind::Method | CodeKind::SuperclassClause | CodeKind::SuperclassMessage => {
                        self.emit(Instr::Return { reg });
                        None
                    }
                    CodeKind::Block => {
                        self.emit(Instr::NonLocalReturn { reg });
                        None
                    }
                    // A top-level `^` in an initializer has no method to
                    // return from: it is evaluated and its value discarded.
                    CodeKind::Initializer => Some(reg),
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
                let reg = match last_value {
                    Some(reg) => reg,
                    None => self.literal(BcLiteral::Nil),
                };
                self.emit(Instr::ImplicitReturn { reg });
            }
            CodeKind::Method
            | CodeKind::Block
            | CodeKind::SuperclassClause
            | CodeKind::SuperclassMessage => {}
        }
    }

    fn return_self(&mut self) {
        let reg = self.fresh();
        self.emit(Instr::LoadSelf { dst: reg });
        self.emit(Instr::ImplicitReturn { reg });
    }

    fn literal(&mut self, literal: BcLiteral) -> u16 {
        let dst = self.fresh();
        self.emit(Instr::LoadLiteral { dst, literal });
        dst
    }

    /// Lowers `expr` and answers the register holding its value -- always a
    /// register this lowering wrote for it and will never write again.
    fn value(&mut self, expr: &Expr) -> u16 {
        match expr {
            Expr::Literal(lit) => self.literal(lower_literal(lit)),
            Expr::Ident(id) => self.read(id),
            Expr::Assign { target, value, .. } => self.assign(target, value),
            Expr::Send(send) => self.send(send),
            Expr::Cascade(cascade) => self.cascade(cascade),
            Expr::Block(block) => self.block(block),
            Expr::DynamicArray { elements, .. } => {
                let elem_regs = self.values(elements);
                let dst = self.fresh();
                self.emit(Instr::MakeArray { dst, elem_regs });
                dst
            }
        }
    }

    /// Left to right, each into a register of its own.
    fn values(&mut self, exprs: &[Expr]) -> Vec<u16> {
        exprs.iter().map(|e| self.value(e)).collect()
    }

    fn read(&mut self, id: &ast::Ident) -> u16 {
        if id.name == "self" {
            let dst = self.fresh();
            self.emit(Instr::LoadSelf { dst });
            return dst;
        }
        match self.binding(id.span) {
            Some(Binding::Local(src)) => {
                let dst = self.fresh();
                self.emit(Instr::Move { dst, src });
                dst
            }
            Some(Binding::OuterLocal { depth, index }) => {
                let dst = self.fresh();
                self.emit(Instr::LoadOuterLocal { dst, depth, index });
                dst
            }
            Some(Binding::LexicalSend { depth }) => {
                let selector = Symbol::intern(&id.name);
                self.emit_send(|dst, call_site| Instr::SendLexical {
                    depth,
                    selector,
                    arg_regs: vec![],
                    dst,
                    call_site,
                })
            }
            // No binding: a bare builtin class name, or the deferred
            // unresolved-identifier error (`docs/design/compiler.md`,
            // "Compilation cannot fail"). A bare identifier bound `SuperSend`
            // is a `super`-prefixed superclass-clause receiver, which falls to
            // the same case.
            Some(Binding::SuperSend) | None => {
                let dst = self.fresh();
                match crate::interpreter::instantiate::builtin_kind_from_name(&id.name) {
                    Some(kind) => self.emit(Instr::LoadBuiltinClass { dst, kind }),
                    None => self.emit(Instr::LoadLiteral {
                        dst,
                        literal: BcLiteral::UnresolvedIdent(id.name.clone()),
                    }),
                }
                dst
            }
        }
    }

    /// The assignment's value is the register its value was computed into,
    /// never the local it was stored in.
    fn assign(&mut self, target: &ast::Ident, value: &Expr) -> u16 {
        let v = self.value(value);
        match self.binding(target.span) {
            Some(Binding::Local(slot)) => self.emit(Instr::StoreLocal { src: v, slot }),
            Some(Binding::OuterLocal { depth, index }) => {
                self.emit(Instr::StoreOuterLocal { src: v, depth, index });
            }
            Some(Binding::LexicalSend { depth }) => {
                let selector = Symbol::intern(&format!("{}:", target.name));
                self.emit_send(|dst, call_site| Instr::SendLexical {
                    depth,
                    selector,
                    arg_regs: vec![v],
                    dst,
                    call_site,
                });
            }
            Some(Binding::SuperSend) | None => panic!(
                "internal: unresolved assignment target `{}` (the resolver should have rejected this)",
                target.name
            ),
        }
        v
    }

    fn send(&mut self, send: &ast::Send) -> u16 {
        let selector = selector_of(&send.message);
        match self.binding(send.span) {
            Some(Binding::SuperSend) => {
                let arg_regs = self.values(&send.message.args);
                self.emit_send(|dst, call_site| Instr::SendSuper {
                    selector,
                    arg_regs,
                    dst,
                    call_site,
                })
            }
            Some(Binding::LexicalSend { depth }) => {
                let arg_regs = self.values(&send.message.args);
                self.emit_send(|dst, call_site| Instr::SendLexical {
                    depth,
                    selector,
                    arg_regs,
                    dst,
                    call_site,
                })
            }
            // An ordinary send. The ones the optimized lowering inlines are
            // marked `SendInlinable`, which only the depth count reads
            // (`docs/design/compiler.md`, "How the limit counts").
            Some(Binding::Local(_) | Binding::OuterLocal { .. }) | None => {
                let receiver_reg = self.value(&send.receiver);
                if control_flow::recognize(send).is_some() {
                    self.inlinable_send(receiver_reg, &send.message)
                } else {
                    self.public_send(receiver_reg, &send.message)
                }
            }
        }
    }

    fn public_send(&mut self, receiver_reg: u16, message: &ast::MessageSend) -> u16 {
        let selector = selector_of(message);
        let arg_regs = self.values(&message.args);
        self.emit_send(|dst, call_site| Instr::SendPublic {
            selector,
            receiver_reg,
            arg_regs,
            dst,
            call_site,
        })
    }

    fn inlinable_send(&mut self, receiver_reg: u16, message: &ast::MessageSend) -> u16 {
        let selector = selector_of(message);
        let arg_regs = self.values(&message.args);
        self.emit_send(|dst, call_site| Instr::SendInlinable {
            selector,
            receiver_reg,
            arg_regs,
            dst,
            call_site,
        })
    }

    fn cascade(&mut self, cascade: &ast::Cascade) -> u16 {
        let receiver_reg = self.value(&cascade.receiver);
        let mut result = self.public_send(receiver_reg, &cascade.first);
        for message in &cascade.rest {
            result = self.public_send(receiver_reg, message);
        }
        result
    }

    fn block(&mut self, block: &ast::BlockExpr) -> u16 {
        let params: Vec<Symbol> = block
            .params
            .iter()
            .map(|p| Symbol::intern(&p.name))
            .collect();
        let mut inner = Body::new(
            CodeKind::Block,
            u16::try_from(params.len())
                .expect("internal: more than u16::MAX parameters in one block"),
            u16::try_from(block.body.temps.len())
                .expect("internal: more than u16::MAX temporaries in one block"),
            self.bindings,
        );
        inner.statements_and_end(&block.body.stmts);
        let code = inner.finish(block.span);
        let dst = self.fresh();
        self.emit(Instr::MakeClosure {
            dst,
            params: Rc::new(params),
            code: Rc::new(code),
        });
        dst
    }

    fn emit_send(&mut self, build: impl FnOnce(u16, CallSite) -> Instr) -> u16 {
        let dst = self.fresh();
        // A call site is its send's own instruction index. Checked in every
        // build for the same reason as `fresh`: a wrapped id would alias two
        // call sites silently.
        let call_site = CallSite {
            call_site_id: u16::try_from(self.instrs.len())
                .expect("internal: more than u16::MAX instructions in one body"),
        };
        self.emit(build(dst, call_site));
        self.call_sites.push(call_site);
        dst
    }

    fn finish(self, source: Span) -> CompiledCode {
        if cfg!(debug_assertions) {
            check_shape(&self.instrs, self.first_scratch);
        }
        CompiledCode {
            kind: self.kind,
            instrs: self.instrs,
            num_args: self.num_args,
            num_registers: self.next_register,
            statement_pc: self.statement_pc,
            call_sites: self.call_sites,
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
fn check_shape(instrs: &[Instr], first_scratch: u16) {
    let mut written = HashSet::new();
    for instr in instrs {
        assert!(
            !matches!(
                instr,
                Instr::Jump { .. }
                    | Instr::JumpIfNotBoolean { .. }
                    | Instr::BranchIfFalse { .. }
                    | Instr::BranchIfTrue { .. }
            ),
            "internal: the reference lowering emitted a branch: {instr:?}"
        );
        if let Some(dst) = destination(instr) {
            assert!(
                dst >= first_scratch,
                "internal: the reference lowering wrote named local r{dst} other than by StoreLocal"
            );
            assert!(
                written.insert(dst),
                "internal: the reference lowering wrote r{dst} twice"
            );
        }
        if let Instr::StoreLocal { slot, .. } = instr {
            assert!(
                *slot < first_scratch,
                "internal: the reference lowering stored into scratch register r{slot}: {instr:?}"
            );
        }
        for operand in operands(instr) {
            match instr {
                Instr::Move { .. } => assert!(
                    operand < first_scratch,
                    "internal: the reference lowering copied scratch register r{operand}: {instr:?}"
                ),
                _ => assert!(
                    operand >= first_scratch,
                    "internal: the reference lowering read named local r{operand} without copying it: {instr:?}"
                ),
            }
        }
    }
}

/// The registers of this body that `instr` reads. Exhaustive on purpose, like
/// `destination`: a new instruction has to decide here what it reads. The
/// `index` of an outer-local access names a register of another frame, so it
/// is not one of them.
fn operands(instr: &Instr) -> Vec<u16> {
    match instr {
        Instr::Move { src, .. }
        | Instr::StoreLocal { src, .. }
        | Instr::StoreOuterLocal { src, .. }
        | Instr::InitSlot { src, .. } => vec![*src],
        Instr::Return { reg }
        | Instr::ImplicitReturn { reg }
        | Instr::NonLocalReturn { reg }
        | Instr::JumpIfNotBoolean { reg, .. }
        | Instr::BranchIfFalse { reg, .. }
        | Instr::BranchIfTrue { reg, .. } => vec![*reg],
        Instr::SendPublic {
            receiver_reg,
            arg_regs,
            ..
        }
        | Instr::SendInlinable {
            receiver_reg,
            arg_regs,
            ..
        } => std::iter::once(*receiver_reg)
            .chain(arg_regs.iter().copied())
            .collect(),
        Instr::SendLexical { arg_regs, .. } | Instr::SendSuper { arg_regs, .. } => arg_regs.clone(),
        Instr::MakeArray { elem_regs, .. } => elem_regs.clone(),
        Instr::LoadLiteral { .. }
        | Instr::LoadSelf { .. }
        | Instr::LoadBuiltinClass { .. }
        | Instr::LoadOuterLocal { .. }
        | Instr::MakeClosure { .. }
        | Instr::Jump { .. } => vec![],
    }
}

/// The register `instr` writes, if any. Exhaustive on purpose: a new
/// instruction has to decide here whether it writes a register.
fn destination(instr: &Instr) -> Option<u16> {
    match instr {
        Instr::LoadLiteral { dst, .. }
        | Instr::LoadSelf { dst }
        | Instr::LoadBuiltinClass { dst, .. }
        | Instr::Move { dst, .. }
        | Instr::LoadOuterLocal { dst, .. }
        | Instr::SendLexical { dst, .. }
        | Instr::SendPublic { dst, .. }
        | Instr::SendInlinable { dst, .. }
        | Instr::SendSuper { dst, .. }
        | Instr::MakeClosure { dst, .. }
        | Instr::MakeArray { dst, .. } => Some(*dst),
        Instr::StoreLocal { .. }
        | Instr::StoreOuterLocal { .. }
        | Instr::InitSlot { .. }
        | Instr::Return { .. }
        | Instr::ImplicitReturn { .. }
        | Instr::NonLocalReturn { .. }
        | Instr::Jump { .. }
        | Instr::JumpIfNotBoolean { .. }
        | Instr::BranchIfFalse { .. }
        | Instr::BranchIfTrue { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Compiles instance-side method `index` of `src` under the reference
    /// lowering, against the bindings the resolver really produces.
    fn compile(src: &str, index: usize) -> CompiledCode {
        let module = ns_syntax::parse(src).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(src),
            crate::SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        compile_method(
            &resolved.module.root_class.instance_side.methods[index],
            &resolved.bindings,
        )
    }

    fn method(decl: &str) -> String {
        format!("Newspeak3\n'T'\nclass X = Object new (\n) (\n  {decl}\n) : ()")
    }

    fn send(selector: &str, receiver_reg: u16, arg_regs: Vec<u16>, dst: u16, id: u16) -> Instr {
        Instr::SendPublic {
            selector: Symbol::intern(selector),
            receiver_reg,
            arg_regs,
            dst,
            call_site: CallSite { call_site_id: id },
        }
    }

    fn inlinable(
        selector: &str,
        receiver_reg: u16,
        arg_regs: Vec<u16>,
        dst: u16,
        id: u16,
    ) -> Instr {
        Instr::SendInlinable {
            selector: Symbol::intern(selector),
            receiver_reg,
            arg_regs,
            dst,
            call_site: CallSite { call_site_id: id },
        }
    }

    /// Exactly the sends `control_flow::recognize` accepts are marked, so the
    /// depth count can exempt the block activations the optimized lowering
    /// inlines away (`docs/design/compiler.md`, "How the limit counts").
    #[test]
    fn a_send_the_optimized_lowering_inlines_is_a_send_inlinable() {
        let code = compile(&method("foo: x = ( ^x ifTrue: [1] )"), 0);
        assert!(
            code.instrs
                .iter()
                .any(|i| matches!(i, Instr::SendInlinable { selector, .. } if selector.as_str() == "ifTrue:")),
            "{code:?}"
        );
        assert!(!code
            .instrs
            .iter()
            .any(|i| matches!(i, Instr::SendPublic { .. })));
    }

    #[test]
    fn a_send_the_optimized_lowering_does_not_inline_stays_a_send_public() {
        // A non-literal block argument, and a cascade message: neither is
        // offered to `recognize` in the optimized lowering.
        for decl in [
            "foo: x = ( ^x ifTrue: x )",
            "foo: x = ( ^x yourself; ifTrue: [1] )",
        ] {
            let code = compile(&method(decl), 0);
            assert!(
                !code
                    .instrs
                    .iter()
                    .any(|i| matches!(i, Instr::SendInlinable { .. })),
                "{decl}: {code:?}"
            );
        }
    }

    #[test]
    fn every_inlinable_selector_is_marked() {
        for (decl, selector) in [
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
            let code = compile(&method(decl), 0);
            assert!(
                code.instrs.iter().any(
                    |i| matches!(i, Instr::SendInlinable { selector: s, .. } if s.as_str() == selector)
                ),
                "{decl}: {code:?}"
            );
        }
    }

    fn closure_code(instr: &Instr) -> &CompiledCode {
        let Instr::MakeClosure { code, .. } = instr else {
            panic!("expected a MakeClosure, got {instr:?}");
        };
        code
    }

    fn no_branch(code: &CompiledCode) -> bool {
        code.instrs.iter().all(|i| {
            !matches!(
                i,
                Instr::Jump { .. }
                    | Instr::JumpIfNotBoolean { .. }
                    | Instr::BranchIfFalse { .. }
                    | Instr::BranchIfTrue { .. }
            )
        })
    }

    #[test]
    fn reading_a_local_copies_it_into_a_fresh_register() {
        let code = compile(&method("foo: x = ( ^x )"), 0);
        assert_eq!(
            code.instrs,
            vec![Instr::Move { dst: 1, src: 0 }, Instr::Return { reg: 1 }]
        );
        assert_eq!(code.num_registers, 2);
    }

    #[test]
    fn no_register_is_reused_across_statements() {
        let code = compile(&method("foo = ( 1. 2 )"), 0);
        assert_eq!(
            code.instrs,
            vec![
                Instr::LoadLiteral {
                    dst: 0,
                    literal: BcLiteral::SmallInt(1)
                },
                Instr::LoadLiteral {
                    dst: 1,
                    literal: BcLiteral::SmallInt(2)
                },
                Instr::LoadSelf { dst: 2 },
                Instr::ImplicitReturn { reg: 2 },
            ]
        );
        assert_eq!(code.statement_pc, vec![(0, 0..1), (1, 1..2)]);
        assert_eq!(code.num_registers, 3);
    }

    #[test]
    fn if_true_if_false_is_a_real_send_with_two_real_closures() {
        let code = compile(&method("foo: x = ( ^x ifTrue: [1] ifFalse: [2] )"), 0);
        assert!(no_branch(&code));
        assert_eq!(code.instrs.len(), 5);
        assert_eq!(code.instrs[0], Instr::Move { dst: 1, src: 0 });
        assert!(matches!(code.instrs[1], Instr::MakeClosure { dst: 2, .. }));
        assert!(matches!(code.instrs[2], Instr::MakeClosure { dst: 3, .. }));
        assert_eq!(
            code.instrs[3],
            inlinable("ifTrue:ifFalse:", 1, vec![2, 3], 4, 3)
        );
        assert_eq!(code.instrs[4], Instr::Return { reg: 4 });
        assert_eq!(
            closure_code(&code.instrs[1]).instrs,
            vec![
                Instr::LoadLiteral {
                    dst: 0,
                    literal: BcLiteral::SmallInt(1)
                },
                Instr::ImplicitReturn { reg: 0 },
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
            code.instrs[0],
            Instr::LoadLiteral {
                dst: 1,
                literal: BcLiteral::SmallInt(0)
            }
        );
        assert_eq!(code.instrs[1], Instr::StoreLocal { src: 1, slot: 0 });
        assert!(matches!(code.instrs[2], Instr::MakeClosure { dst: 2, .. }));
        assert!(matches!(code.instrs[3], Instr::MakeClosure { dst: 3, .. }));
        assert_eq!(code.instrs[4], inlinable("whileTrue:", 2, vec![3], 4, 4));
        assert_eq!(code.instrs[5], Instr::Move { dst: 5, src: 0 });
        assert_eq!(code.instrs[6], Instr::Return { reg: 5 });
        assert_eq!(code.statement_pc, vec![(0, 0..2), (1, 2..5), (2, 5..7)]);
        assert_eq!(
            closure_code(&code.instrs[2]).instrs,
            vec![
                Instr::LoadOuterLocal {
                    dst: 0,
                    depth: 1,
                    index: 0
                },
                Instr::LoadLiteral {
                    dst: 1,
                    literal: BcLiteral::SmallInt(3)
                },
                send("<", 0, vec![1], 2, 2),
                Instr::ImplicitReturn { reg: 2 },
            ]
        );
        assert_eq!(
            closure_code(&code.instrs[3]).instrs,
            vec![
                Instr::LoadOuterLocal {
                    dst: 0,
                    depth: 1,
                    index: 0
                },
                Instr::LoadLiteral {
                    dst: 1,
                    literal: BcLiteral::SmallInt(1)
                },
                send("+", 0, vec![1], 2, 2),
                Instr::StoreOuterLocal {
                    src: 2,
                    depth: 1,
                    index: 0
                },
                Instr::ImplicitReturn { reg: 2 },
            ]
        );
    }

    #[test]
    fn a_return_inside_a_block_the_optimized_lowering_inlines_is_a_non_local_return() {
        let code = compile(&method("foo: x = ( x ifTrue: [^1]. ^2 )"), 0);
        let block = closure_code(&code.instrs[1]);
        assert_eq!(block.kind, CodeKind::Block);
        assert_eq!(
            block.instrs,
            vec![
                Instr::LoadLiteral {
                    dst: 0,
                    literal: BcLiteral::SmallInt(1)
                },
                Instr::NonLocalReturn { reg: 0 },
            ]
        );
    }

    #[test]
    fn an_assignments_value_is_its_fresh_register_never_the_local() {
        let code = compile(&method("foo = ( | t | ^t := 3 )"), 0);
        assert_eq!(
            code.instrs,
            vec![
                Instr::LoadLiteral {
                    dst: 1,
                    literal: BcLiteral::SmallInt(3)
                },
                Instr::StoreLocal { src: 1, slot: 0 },
                Instr::Return { reg: 1 },
            ]
        );
    }

    #[test]
    fn a_cascade_evaluates_its_receiver_once_and_answers_the_last_send() {
        let code = compile(&method("foo: x = ( ^x foo; bar )"), 0);
        assert_eq!(
            code.instrs,
            vec![
                Instr::Move { dst: 1, src: 0 },
                send("foo", 1, vec![], 2, 1),
                send("bar", 1, vec![], 3, 2),
                Instr::Return { reg: 3 },
            ]
        );
    }

    /// `check_shape` itself: each rule refuses a body that breaks it. One
    /// named local (`r0`), so `r1` is the first scratch register.
    #[test]
    #[should_panic(expected = "read named local r0 without copying it")]
    fn the_shape_check_refuses_a_named_local_read_other_than_by_move() {
        check_shape(&[Instr::Return { reg: 0 }], 1);
    }

    #[test]
    #[should_panic(expected = "copied scratch register r1")]
    fn the_shape_check_refuses_a_move_out_of_a_scratch_register() {
        check_shape(
            &[Instr::LoadSelf { dst: 1 }, Instr::Move { dst: 2, src: 1 }],
            1,
        );
    }

    #[test]
    #[should_panic(expected = "stored into scratch register r1")]
    fn the_shape_check_refuses_a_store_into_a_scratch_register() {
        check_shape(
            &[
                Instr::LoadSelf { dst: 2 },
                Instr::StoreLocal { src: 2, slot: 1 },
            ],
            1,
        );
    }

    #[test]
    fn the_shape_check_accepts_a_copied_read_of_a_named_local() {
        check_shape(
            &[
                Instr::Move { dst: 1, src: 0 },
                Instr::StoreLocal { src: 1, slot: 0 },
                Instr::Return { reg: 1 },
            ],
            1,
        );
    }

    #[test]
    fn an_empty_block_answers_nil() {
        let code = compile(&method("foo = ( ^[] )"), 0);
        assert_eq!(
            closure_code(&code.instrs[0]).instrs,
            vec![
                Instr::LoadLiteral {
                    dst: 0,
                    literal: BcLiteral::Nil
                },
                Instr::ImplicitReturn { reg: 0 },
            ]
        );
    }
}
