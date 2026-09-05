//! `Expr`/`Stmt` -> `Instr` lowering, register allocation, and the scope
//! stack that maps resolver depths onto runtime frame hops. See
//! `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md`'s
//! `bytecode.rs` and "Control-flow inlining" sections.
//!
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use ns_syntax::ast::{self, Expr, Stmt};
use ns_syntax::Span;

use super::control_flow::{self, InlineForm};
use crate::bytecode::{CallSite, CodeKind, CompiledCode, Instr, Literal as BcLiteral};
use crate::interpreter::eval::parse_int_literal;
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

/// The runtime `captured_frame` hop count for a resolver depth of `depth`
/// scopes: the number of `Frame` scopes among the innermost `depth`
/// entries of `scopes` (innermost last).
pub(crate) fn hops_for_depth(scopes: &[ScopeKind], depth: u16) -> u16 {
    scopes
        .iter()
        .rev()
        .take(depth as usize)
        .filter(|s| **s == ScopeKind::Frame)
        .count() as u16
}

pub(crate) struct Lowerer<'a> {
    bindings: &'a Rc<HashMap<Span, Binding>>,
    kind: CodeKind,
    num_args: u16,
    /// Innermost last. The last entry is this body's own `Frame` scope
    /// with `Inlined` entries pushed above it while an inlined block body
    /// is being lowered.
    scopes: Vec<ScopeKind>,
    instrs: Vec<Instr>,
    base: u16,
    next_scratch: u16,
    max_reg: u16,
    statement_index: usize,
    statement_pc: Vec<(usize, Range<usize>)>,
    call_sites: Vec<CallSite>,
    /// The most recent top-level `Stmt::Expr`'s value register, for
    /// `finish_body`'s block case and for reading an inlined
    /// block's result.
    last_expr_reg: Option<u16>,
}

impl<'a> Lowerer<'a> {
    pub(crate) fn new(
        kind: CodeKind,
        num_args: u16,
        num_named_temps: u16,
        bindings: &'a Rc<HashMap<Span, Binding>>,
        mut enclosing_scopes: Vec<ScopeKind>,
    ) -> Self {
        enclosing_scopes.push(ScopeKind::Frame);
        let base = num_args + num_named_temps;
        Lowerer {
            bindings,
            kind,
            num_args,
            scopes: enclosing_scopes,
            instrs: Vec::new(),
            base,
            next_scratch: base,
            max_reg: base,
            statement_index: 0,
            statement_pc: Vec::new(),
            call_sites: Vec::new(),
            last_expr_reg: None,
        }
    }

    fn alloc_scratch(&mut self) -> u16 {
        let r = self.next_scratch;
        self.next_scratch += 1;
        self.max_reg = self.max_reg.max(self.next_scratch);
        r
    }

    fn emit(&mut self, instr: Instr) -> usize {
        self.instrs.push(instr);
        self.instrs.len() - 1
    }

    /// Lowers one of the body's own statements, recording its safepoint
    /// range in `statement_pc` and resetting the scratch allocator first
    /// (statements are the safepoint/scratch-liveness boundary).
    pub(crate) fn lower_statement(&mut self, stmt: &Stmt) {
        self.next_scratch = self.base;
        let start = self.instrs.len();
        self.lower_stmt_inner(stmt);
        let end = self.instrs.len();
        self.statement_pc.push((self.statement_index, start..end));
        self.statement_index += 1;
    }

    /// The initializer's per-slot statement: the slot's initializer
    /// expression, then `InitSlot` (M1's `run_own_slots` writes the field
    /// directly, never through a setter send). One `statement_pc` entry.
    pub(crate) fn lower_slot_init(&mut self, init: &Expr, field_index: u32) {
        self.next_scratch = self.base;
        let start = self.instrs.len();
        let src = self.lower_expr(init);
        self.emit(Instr::InitSlot { src, field_index });
        let end = self.instrs.len();
        self.statement_pc.push((self.statement_index, start..end));
        self.statement_index += 1;
    }

    /// Lowers a statement without touching the allocator or the PC table --
    /// shared by `lower_statement` and `lower_inline_block_body`.
    /// Returns the value register for a `Stmt::Expr` (or a discarded
    /// initializer `^`), `None` when a return instruction was emitted.
    fn lower_stmt_inner(&mut self, stmt: &Stmt) -> Option<u16> {
        match stmt {
            Stmt::Expr(e) => {
                let reg = self.lower_expr(e);
                self.last_expr_reg = Some(reg);
                Some(reg)
            }
            Stmt::Return(e) => {
                let reg = self.lower_expr(e);
                match self.return_instr(reg) {
                    Some(instr) => {
                        self.emit(instr);
                        self.last_expr_reg = None;
                        None
                    }
                    None => {
                        self.last_expr_reg = Some(reg);
                        Some(reg)
                    }
                }
            }
        }
    }

    /// What a `^` means here -- the spec's per-code-kind table. `None` is
    /// "evaluate and discard" (a top-level `^` in `initExprs`, exactly as
    /// M1's `run_own_init_exprs`). Inside an inlined block (`scopes.last()
    /// == Inlined`) an initializer's `^` must instead raise the
    /// same `Signal::Return` M1's real block would, so it becomes
    /// `NonLocalReturn`.
    fn return_instr(&self, reg: u16) -> Option<Instr> {
        let inside_inlined = self.scopes.last() == Some(&ScopeKind::Inlined);
        match (self.kind, inside_inlined) {
            (CodeKind::Method, _) => Some(Instr::Return { reg }),
            (CodeKind::Block, _) => Some(Instr::NonLocalReturn { reg }),
            (CodeKind::Initializer, false) => None,
            (CodeKind::Initializer, true) => Some(Instr::NonLocalReturn { reg }),
        }
    }

    fn emit_nil(&mut self) -> u16 {
        let dst = self.alloc_scratch();
        self.emit(Instr::LoadLiteral {
            dst,
            literal: BcLiteral::Nil,
        });
        dst
    }

    /// Appends the body's terminating instruction if `stmts` (the body just
    /// lowered) did not already end in one, so `vm::run_code` never has to
    /// handle "fell off the end". A method returns `self`; an initializer
    /// always returns `self` (its top-level `^` never emits a return); a
    /// block returns its last statement's value, `nil` if it has none.
    pub(crate) fn finish_body(&mut self, stmts: &[Stmt]) {
        let ends_in_return = matches!(stmts.last(), Some(Stmt::Return(_)));
        match self.kind {
            CodeKind::Method if ends_in_return => {}
            CodeKind::Method | CodeKind::Initializer => {
                let dst = self.alloc_scratch();
                self.emit(Instr::LoadSelf { dst });
                self.emit(Instr::ImplicitReturn { reg: dst });
            }
            CodeKind::Block if ends_in_return => {}
            CodeKind::Block => {
                let reg = match self.last_expr_reg {
                    Some(r) => r,
                    None => self.emit_nil(),
                };
                self.emit(Instr::ImplicitReturn { reg });
            }
        }
    }

    /// Compiles a block literal into its own `CompiledCode` with a nested
    /// `Lowerer` that inherits this one's whole scope stack, and emits
    /// `MakeClosure`. `captured_frame`/`home`/`bindings` are supplied by the
    /// VM at run time (spec: "Blocks and closures").
    fn lower_block_literal(&mut self, block: &ast::BlockExpr) -> u16 {
        let params: Vec<crate::Symbol> = block
            .params
            .iter()
            .map(|p| crate::Symbol::intern(&p.name))
            .collect();
        let mut inner = Lowerer::new(
            CodeKind::Block,
            params.len() as u16,
            block.body.temps.len() as u16,
            self.bindings,
            self.scopes.clone(),
        );
        for stmt in &block.body.stmts {
            inner.lower_statement(stmt);
        }
        inner.finish_body(&block.body.stmts);
        let code = inner.finish(block.span);
        let dst = self.alloc_scratch();
        self.emit(Instr::MakeClosure {
            dst,
            params: Rc::new(params),
            code: Rc::new(code),
        });
        dst
    }

    /// Returns the register holding `expr`'s value.
    pub(crate) fn lower_expr(&mut self, expr: &Expr) -> u16 {
        match expr {
            Expr::Literal(lit) => {
                let dst = self.alloc_scratch();
                self.emit(Instr::LoadLiteral {
                    dst,
                    literal: lower_literal(lit),
                });
                dst
            }
            Expr::Ident(id) => self.lower_ident_read(id),
            Expr::Assign { target, value, .. } => self.lower_assign(target, value),
            Expr::Send(send) => self.lower_send(send),
            Expr::Cascade(cascade) => self.lower_cascade(cascade),
            Expr::Block(block) => self.lower_block_literal(block),
            Expr::DynamicArray { elements, .. } => {
                let elem_regs: Vec<u16> = elements.iter().map(|e| self.lower_expr(e)).collect();
                let dst = self.alloc_scratch();
                self.emit(Instr::MakeArray { dst, elem_regs });
                dst
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

    fn lower_ident_read(&mut self, id: &ast::Ident) -> u16 {
        if id.name == "self" {
            let dst = self.alloc_scratch();
            self.emit(Instr::LoadSelf { dst });
            return dst;
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
                let dst = self.alloc_scratch();
                self.emit(Instr::LoadOuterLocal {
                    dst,
                    depth: hops,
                    index,
                });
                dst
            }
            Some(Binding::LexicalSend { depth }) => {
                let depth = *depth;
                self.emit_lexical_send(depth, crate::Symbol::intern(&id.name), vec![])
            }
            // M1's resolver never attaches `SuperSend` to a bare identifier (only to
            // a `super m` send's own span) -- grouped with `None` here because both
            // fall to the same builtin-name fallback, mirroring `eval_ident`.
            Some(Binding::SuperSend) | None => {
                match crate::interpreter::instantiate::builtin_kind_from_name(&id.name) {
                    Some(kind) => {
                        let dst = self.alloc_scratch();
                        self.emit(Instr::LoadBuiltinClass { dst, kind });
                        dst
                    }
                    None => panic!(
                        "internal: unresolved identifier `{}` (the resolver should have rejected this)",
                        id.name
                    ),
                }
            }
        }
    }

    fn lower_assign(&mut self, target: &ast::Ident, value: &Expr) -> u16 {
        let v = self.lower_expr(value);
        match self.bindings.get(&target.span) {
            Some(Binding::Local(slot)) => {
                let (_, index) = self.local_target(0, *slot);
                self.emit(Instr::StoreLocal {
                    src: v,
                    slot: index,
                });
            }
            Some(Binding::OuterLocal { depth, index }) => {
                let (hops, index) = self.local_target(*depth, *index);
                if hops == 0 {
                    self.emit(Instr::StoreLocal {
                        src: v,
                        slot: index,
                    });
                } else {
                    self.emit(Instr::StoreOuterLocal {
                        src: v,
                        depth: hops,
                        index,
                    });
                }
            }
            Some(Binding::LexicalSend { depth }) => {
                let depth = *depth;
                let setter = crate::Symbol::intern(&format!("{}:", target.name));
                self.emit_lexical_send(depth, setter, vec![v]);
            }
            Some(Binding::SuperSend) | None => panic!(
                "internal: unresolved setter target `{}` (the resolver should have rejected this)",
                target.name
            ),
        }
        // An assignment's own value is `value`'s value (runtime spec's
        // "Setter sends" note) -- already sitting in register `v`.
        v
    }

    /// One `Send*` instruction's `CallSite`: the instruction's own
    /// about-to-be-pushed index, always in sync with `instrs`.
    fn next_call_site(&self) -> CallSite {
        // `call_site_id` is `u16` -- spec-mandated, Milestone 3's
        // inline-cache slot -- so a body of more than 65535 instructions
        // would silently alias two call sites onto the same id. Inlining
        // raises instructions-per-body, so this is no longer purely
        // theoretical; make the ceiling an explicit assertion rather than a
        // silent truncation.
        debug_assert!(
            self.instrs.len() < u16::MAX as usize,
            "internal: more than 65535 instructions in one body -- call_site_id (u16) would alias"
        );
        CallSite {
            call_site_id: self.instrs.len() as u16,
        }
    }

    fn emit_send(&mut self, build: impl FnOnce(u16, CallSite) -> Instr) -> u16 {
        let dst = self.alloc_scratch();
        let call_site = self.next_call_site();
        self.emit(build(dst, call_site));
        self.call_sites.push(call_site);
        dst
    }

    fn emit_lexical_send(
        &mut self,
        depth: u16,
        selector: crate::Symbol,
        arg_regs: Vec<u16>,
    ) -> u16 {
        self.emit_send(|dst, call_site| Instr::SendLexical {
            depth,
            selector,
            arg_regs,
            dst,
            call_site,
        })
    }

    pub(crate) fn emit_ordinary_send_with_receiver(
        &mut self,
        receiver_reg: u16,
        message: &ast::MessageSend,
    ) -> u16 {
        let selector = selector_of(message);
        let arg_regs = self.lower_args(message);
        self.emit_send(|dst, call_site| Instr::SendPublic {
            selector,
            receiver_reg,
            arg_regs,
            dst,
            call_site,
        })
    }

    fn lower_args(&mut self, message: &ast::MessageSend) -> Vec<u16> {
        message.args.iter().map(|a| self.lower_expr(a)).collect()
    }

    fn lower_send(&mut self, send: &ast::Send) -> u16 {
        match self.bindings.get(&send.span) {
            Some(Binding::SuperSend) => {
                let selector = selector_of(&send.message);
                let arg_regs = self.lower_args(&send.message);
                self.emit_send(|dst, call_site| Instr::SendSuper {
                    selector,
                    arg_regs,
                    dst,
                    call_site,
                })
            }
            Some(Binding::LexicalSend { depth }) => {
                let depth = *depth;
                let selector = selector_of(&send.message);
                let arg_regs = self.lower_args(&send.message);
                self.emit_lexical_send(depth, selector, arg_regs)
            }
            _ => {
                // The inlining gate: only an ordinary explicit-receiver send
                // is ever offered to `recognize` (M2 spec, "Control-flow
                // inlining"). Every recognized form still keeps a runtime
                // guard whose failure path is a real virtual send, so
                // inlining stays a pure optimization.
                if let Some(form) = control_flow::recognize(send) {
                    return match form {
                        InlineForm::IfTrueIfFalse(t, f) => self.lower_if(send, Some(t), Some(f)),
                        InlineForm::IfTrue(t) => self.lower_if(send, Some(t), None),
                        InlineForm::IfFalse(f) => self.lower_if(send, None, Some(f)),
                        InlineForm::And(b) => self.lower_and_or(send, b, true),
                        InlineForm::Or(b) => self.lower_and_or(send, b, false),
                        InlineForm::WhileTrue { cond, body } => self.lower_while(cond, body, true),
                        InlineForm::WhileFalse { cond, body } => {
                            self.lower_while(cond, body, false)
                        }
                    };
                }
                let receiver_reg = self.lower_expr(&send.receiver);
                self.emit_ordinary_send_with_receiver(receiver_reg, &send.message)
            }
        }
    }

    fn lower_cascade(&mut self, cascade: &ast::Cascade) -> u16 {
        let receiver_reg = self.lower_expr(&cascade.receiver);
        let mut result = self.emit_ordinary_send_with_receiver(receiver_reg, &cascade.first);
        for m in &cascade.rest {
            result = self.emit_ordinary_send_with_receiver(receiver_reg, m);
        }
        result
    }

    /// The index the next emitted instruction will get -- a forward label.
    fn here(&self) -> u32 {
        self.instrs.len() as u32
    }

    fn patch_target(&mut self, at: usize, target: u32) {
        match &mut self.instrs[at] {
            Instr::Jump { target: t }
            | Instr::JumpIfNotBoolean { target: t, .. }
            | Instr::BranchIfFalse { target: t, .. }
            | Instr::BranchIfTrue { target: t, .. } => *t = target,
            other => unreachable!("internal: patch_target on a non-jump instruction {other:?}"),
        }
    }

    /// Lowers `block`'s statements directly into the current instruction
    /// stream -- no `MakeClosure`, no new activation. The block's statements
    /// are sub-expressions of the enclosing statement: no `statement_pc`
    /// entries, and the scratch allocator resets only to the watermark at the
    /// block's start (never to `base`, which would clobber the enclosing
    /// statement's live registers). Pushes `ScopeKind::Inlined` so local
    /// references inside (and in nested real blocks) drop one frame hop.
    /// Returns the register holding the block's value (`nil` if it has no
    /// statements).
    fn lower_inline_block_body(&mut self, block: &ast::BlockExpr) -> u16 {
        debug_assert!(
            block.params.is_empty() && block.body.temps.is_empty(),
            "internal: the inlining gate admits only parameterless, temp-less blocks"
        );
        self.scopes.push(ScopeKind::Inlined);
        let mark = self.next_scratch;
        let mut result = None;
        for stmt in &block.body.stmts {
            self.next_scratch = mark;
            result = self.lower_stmt_inner(stmt);
        }
        self.scopes.pop();
        match result {
            Some(reg) => reg,
            None => self.emit_nil(),
        }
    }

    /// `builtins::boolean::truthy_arg`'s rule in bytecode: only exactly
    /// `Bool(true)` is true; a non-`true` `Bool` or a non-`Bool` is false.
    fn emit_coerce_to_bool(&mut self, src: u16) -> u16 {
        let dst = self.alloc_scratch();
        let not_bool_jump = self.emit(Instr::JumpIfNotBoolean {
            reg: src,
            target: 0,
        });
        let false_jump = self.emit(Instr::BranchIfFalse {
            reg: src,
            target: 0,
        });
        self.emit(Instr::LoadLiteral {
            dst,
            literal: BcLiteral::Bool(true),
        });
        let after_true_jump = self.emit(Instr::Jump { target: 0 });
        let false_label = self.here();
        self.patch_target(not_bool_jump, false_label);
        self.patch_target(false_jump, false_label);
        self.emit(Instr::LoadLiteral {
            dst,
            literal: BcLiteral::Bool(false),
        });
        let end_label = self.here();
        self.patch_target(after_true_jump, end_label);
        dst
    }

    /// The guard-failure path of an inlined loop: what the native
    /// `whileTrue:`/`whileFalse:` does with a non-Boolean condition value.
    fn emit_must_be_boolean(&mut self, reg: u16) -> u16 {
        let selector = crate::Symbol::intern("mustBeBoolean");
        self.emit_send(|dst, call_site| Instr::SendPublic {
            selector,
            receiver_reg: reg,
            arg_regs: vec![],
            dst,
            call_site,
        })
    }

    /// The shared shape behind every one-shot inlined form: `guard_reg`
    /// exactly `Bool(true)` runs `on_true`, exactly `Bool(false)` runs
    /// `on_false`, anything else runs `on_fallback` (a real send). Each
    /// returns the register holding its value; the chosen one is `Move`d into
    /// `dst`.
    ///
    /// The three arms are mutually exclusive -- at most one of them ever
    /// executes for a given evaluation -- so the scratch allocator is reset
    /// to its pre-arm mark before each one, letting arms reuse the same
    /// registers instead of stacking their high-water marks. `guard_reg` and
    /// `dst` (and any already-evaluated sibling register, e.g. another
    /// argument of the enclosing send) are all allocated before `mark` is
    /// taken, so the reset can never touch a register that outlives its arm.
    fn emit_guarded_branch(
        &mut self,
        guard_reg: u16,
        dst: u16,
        on_true: impl FnOnce(&mut Self) -> u16,
        on_false: impl FnOnce(&mut Self) -> u16,
        on_fallback: impl FnOnce(&mut Self) -> u16,
    ) {
        let mark = self.next_scratch;
        let not_bool_jump = self.emit(Instr::JumpIfNotBoolean {
            reg: guard_reg,
            target: 0,
        });
        let false_jump = self.emit(Instr::BranchIfFalse {
            reg: guard_reg,
            target: 0,
        });
        self.next_scratch = mark;
        let true_result = on_true(self);
        self.emit(Instr::Move {
            dst,
            src: true_result,
        });
        let after_true_jump = self.emit(Instr::Jump { target: 0 });
        let false_label = self.here();
        self.patch_target(false_jump, false_label);
        self.next_scratch = mark;
        let false_result = on_false(self);
        self.emit(Instr::Move {
            dst,
            src: false_result,
        });
        let after_false_jump = self.emit(Instr::Jump { target: 0 });
        let fallback_label = self.here();
        self.patch_target(not_bool_jump, fallback_label);
        self.next_scratch = mark;
        let fallback_result = on_fallback(self);
        self.emit(Instr::Move {
            dst,
            src: fallback_result,
        });
        let end_label = self.here();
        self.patch_target(after_true_jump, end_label);
        self.patch_target(after_false_jump, end_label);
        self.next_scratch = mark;
    }

    /// `ifTrue:`/`ifFalse:`/`ifTrue:ifFalse:`/`ifFalse:ifTrue:`: an absent
    /// branch is `nil`, matching `builtins::boolean`'s one-armed arms.
    fn lower_if(
        &mut self,
        send: &ast::Send,
        true_block: Option<&ast::BlockExpr>,
        false_block: Option<&ast::BlockExpr>,
    ) -> u16 {
        let guard_reg = self.lower_expr(&send.receiver);
        let dst = self.alloc_scratch();
        self.emit_guarded_branch(
            guard_reg,
            dst,
            |lw| match true_block {
                Some(b) => lw.lower_inline_block_body(b),
                None => lw.emit_nil(),
            },
            |lw| match false_block {
                Some(b) => lw.lower_inline_block_body(b),
                None => lw.emit_nil(),
            },
            |lw| lw.emit_ordinary_send_with_receiver(guard_reg, &send.message),
        );
        dst
    }

    /// `and:`/`or:`: the short-circuit arm is the receiver's own Boolean
    /// value, the other arm the block's value coerced by `truthy_arg`'s rule.
    fn lower_and_or(&mut self, send: &ast::Send, block: &ast::BlockExpr, is_and: bool) -> u16 {
        let guard_reg = self.lower_expr(&send.receiver);
        let dst = self.alloc_scratch();
        let evaluate_and_coerce = |lw: &mut Self| {
            let b = lw.lower_inline_block_body(block);
            lw.emit_coerce_to_bool(b)
        };
        let fallback =
            |lw: &mut Self| lw.emit_ordinary_send_with_receiver(guard_reg, &send.message);
        if is_and {
            self.emit_guarded_branch(guard_reg, dst, evaluate_and_coerce, |_| guard_reg, fallback);
        } else {
            self.emit_guarded_branch(guard_reg, dst, |_| guard_reg, evaluate_and_coerce, fallback);
        }
        dst
    }

    /// A real loop: condition, guard, exit branch, body, back-edge. A
    /// non-Boolean condition value takes the `mustBeBoolean` path and ends the
    /// loop with that send's value; a normal exit leaves `nil` in `dst`.
    fn lower_while(
        &mut self,
        cond: &ast::BlockExpr,
        body: &ast::BlockExpr,
        while_true: bool,
    ) -> u16 {
        let dst = self.emit_nil();
        let loop_start = self.here();
        let guard_reg = self.lower_inline_block_body(cond);
        let not_bool_jump = self.emit(Instr::JumpIfNotBoolean {
            reg: guard_reg,
            target: 0,
        });
        let exit_jump = if while_true {
            self.emit(Instr::BranchIfFalse {
                reg: guard_reg,
                target: 0,
            })
        } else {
            self.emit(Instr::BranchIfTrue {
                reg: guard_reg,
                target: 0,
            })
        };
        self.lower_inline_block_body(body);
        self.emit(Instr::Jump { target: loop_start });
        let fallback_label = self.here();
        self.patch_target(not_bool_jump, fallback_label);
        let fallback_result = self.emit_must_be_boolean(guard_reg);
        self.emit(Instr::Move {
            dst,
            src: fallback_result,
        });
        let end_label = self.here();
        self.patch_target(exit_jump, end_label);
        dst
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
            self.instrs
                .iter()
                .filter_map(|i| match i {
                    Instr::Jump { target }
                    | Instr::JumpIfNotBoolean { target, .. }
                    | Instr::BranchIfFalse { target, .. }
                    | Instr::BranchIfTrue { target, .. } => Some(*target as usize),
                    _ => None,
                })
                .all(|t| t < self.instrs.len()),
            "internal: a jump target does not name an instruction of this body"
        );
        CompiledCode {
            kind: self.kind,
            instrs: self.instrs,
            num_args: self.num_args,
            num_registers: self.max_reg,
            statement_pc: self.statement_pc,
            call_sites: self.call_sites,
            source,
        }
    }
}

pub(crate) fn selector_of(message: &ast::MessageSend) -> crate::Symbol {
    match &message.selector {
        ast::Selector::Unary(s) | ast::Selector::Binary(s) | ast::Selector::Keyword(s) => {
            crate::Symbol::intern(s)
        }
    }
}

fn lower_literal(lit: &ast::Literal) -> BcLiteral {
    match lit {
        // Parsed once, here, with the shared M1 parser. A literal it rejects
        // is a Newspeak-program error M1 raises at evaluation time, so it is
        // deferred (`MalformedInt`), never a compile-time panic.
        ast::Literal::Int { text, .. } => match parse_int_literal(text) {
            Ok(n) => BcLiteral::SmallInt(n),
            Err(_) => BcLiteral::MalformedInt(text.clone()),
        },
        ast::Literal::Float { text, .. } => BcLiteral::Float(text.parse().unwrap_or(f64::NAN)),
        ast::Literal::String { value, .. } => BcLiteral::Str(value.clone()),
        ast::Literal::Symbol { value, .. } => BcLiteral::Sym(crate::Symbol::intern(value)),
        ast::Literal::Char { value, .. } => BcLiteral::Char(*value),
        ast::Literal::Bool { value, .. } => BcLiteral::Bool(*value),
        ast::Literal::Nil { .. } => BcLiteral::Nil,
        ast::Literal::Array { elements, .. } => {
            BcLiteral::Array(elements.iter().map(lower_literal).collect())
        }
        ast::Literal::Pattern { .. } => BcLiteral::Nil, // Non-Goal, matches eval_literal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Parses `src` (a full, minimal `class ... ( ) ( <methods> ) : ()`
    /// module), resolves it for real via `crate::resolve`, and returns the
    /// module's instance-side method `index` plus the real bindings map it
    /// resolved under -- the same "parse a throwaway module" pattern
    /// `dispatch.rs`'s own tests use, so these tests exercise real
    /// `Binding`s rather than hand-built ones.
    pub(super) fn resolve_method(
        src: &str,
        index: usize,
    ) -> (ast::MethodDecl, Rc<HashMap<Span, Binding>>) {
        let module = ns_syntax::parse(src).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(src),
            crate::SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let method = resolved.module.root_class.instance_side.methods[index].clone();
        (method, resolved.bindings.clone())
    }

    pub(super) fn method_lowerer<'a>(
        method: &ast::MethodDecl,
        bindings: &'a Rc<HashMap<Span, Binding>>,
    ) -> Lowerer<'a> {
        let num_args = match &method.pattern {
            ast::MessagePattern::Unary { .. } => 0,
            ast::MessagePattern::Binary { .. } => 1,
            ast::MessagePattern::Keyword { parts } => parts.len() as u16,
        };
        Lowerer::new(
            CodeKind::Method,
            num_args,
            method.body.temps.len() as u16,
            bindings,
            vec![],
        )
    }

    fn return_expr(method: &ast::MethodDecl) -> &Expr {
        let Stmt::Return(expr) = &method.body.stmts[0] else {
            panic!("expected a return statement");
        };
        expr
    }

    #[test]
    fn a_literal_returns_a_fresh_scratch_register() {
        let (method, bindings) = resolve_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^42 )\n) : ()",
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings); // 0 params, 0 temps -> base 0
        let reg = lowerer.lower_expr(return_expr(&method));
        assert_eq!(reg, 0);
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instrs,
            vec![Instr::LoadLiteral {
                dst: 0,
                literal: BcLiteral::SmallInt(42)
            }]
        );
        assert_eq!(code.num_registers, 1);
        assert_eq!(code.kind, CodeKind::Method);
    }

    #[test]
    fn reading_a_parameter_returns_its_own_register_with_no_extra_instruction() {
        let (method, bindings) = resolve_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo: x = ( ^x )\n) : ()",
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings); // 1 param (register 0)
        let reg = lowerer.lower_expr(return_expr(&method));
        assert_eq!(reg, 0, "reading a param should return its own register");
        let code = lowerer.finish(method.span);
        assert!(
            code.instrs.is_empty(),
            "no instruction needed to read a param"
        );
        assert_eq!(code.num_args, 1);
    }

    #[test]
    fn assigning_a_named_temp_emits_store_local_and_records_the_statement_range() {
        let (method, bindings) = resolve_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( | t | t := 7. ^t )\n) : ()",
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings); // temp `t` is register 0, base 1
        lowerer.lower_statement(&method.body.stmts[0]); // `t := 7.`
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instrs,
            vec![
                Instr::LoadLiteral {
                    dst: 1,
                    literal: BcLiteral::SmallInt(7)
                },
                Instr::StoreLocal { src: 1, slot: 0 },
            ]
        );
        assert_eq!(code.num_registers, 2);
        assert_eq!(code.statement_pc, vec![(0, 0..2)]);
    }

    #[test]
    fn scratch_registers_reset_between_statements() {
        let (method, bindings) = resolve_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( | t | t := 1. t := 2. ^t )\n) : ()",
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings);
        lowerer.lower_statement(&method.body.stmts[0]);
        lowerer.lower_statement(&method.body.stmts[1]);
        let code = lowerer.finish(method.span);
        // Both literals land in scratch register 1 -- the allocator reset
        // between statements instead of growing.
        assert_eq!(
            code.instrs,
            vec![
                Instr::LoadLiteral {
                    dst: 1,
                    literal: BcLiteral::SmallInt(1)
                },
                Instr::StoreLocal { src: 1, slot: 0 },
                Instr::LoadLiteral {
                    dst: 1,
                    literal: BcLiteral::SmallInt(2)
                },
                Instr::StoreLocal { src: 1, slot: 0 },
            ]
        );
        assert_eq!(code.num_registers, 2);
        assert_eq!(code.statement_pc, vec![(0, 0..2), (1, 2..4)]);
    }

    #[test]
    fn an_out_of_range_integer_literal_lowers_to_malformed_int_not_a_panic() {
        let (method, bindings) = resolve_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^99999999999999999999 )\n) : ()",
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings);
        lowerer.lower_expr(return_expr(&method));
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instrs,
            vec![Instr::LoadLiteral {
                dst: 0,
                literal: BcLiteral::MalformedInt("99999999999999999999".to_string()),
            }]
        );
    }

    #[test]
    fn a_radix_literal_is_parsed_at_compile_time() {
        let (method, bindings) = resolve_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^16rFF )\n) : ()",
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings);
        lowerer.lower_expr(return_expr(&method));
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instrs,
            vec![Instr::LoadLiteral {
                dst: 0,
                literal: BcLiteral::SmallInt(255)
            }]
        );
    }

    #[test]
    fn an_ordinary_send_evaluates_receiver_then_args() {
        let (method, bindings) = resolve_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo: x = ( ^3 + x )\n) : ()",
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings);
        let dst = lowerer.lower_expr(return_expr(&method));
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instrs,
            vec![
                Instr::LoadLiteral {
                    dst: 1,
                    literal: BcLiteral::SmallInt(3)
                },
                Instr::SendPublic {
                    selector: crate::Symbol::intern("+"),
                    receiver_reg: 1,
                    arg_regs: vec![0],
                    dst: 2,
                    call_site: CallSite { call_site_id: 1 },
                },
            ]
        );
        assert_eq!(dst, 2);
        assert_eq!(code.num_registers, 3);
        assert_eq!(code.call_sites, vec![CallSite { call_site_id: 1 }]);
    }

    #[test]
    fn a_bare_implicit_send_lowers_to_send_lexical() {
        // `foo`'s body (`^bar`) resolves `bar` as an implicit-receiver unary
        // send at depth 0 (`bar` is a sibling method) -- `Binding::LexicalSend`.
        let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  bar = ( ^42 )\n  foo = ( ^bar )\n) : ()";
        let (foo, bindings) = resolve_method(src, 1);
        let mut lowerer = method_lowerer(&foo, &bindings);
        let dst = lowerer.lower_expr(return_expr(&foo));
        let code = lowerer.finish(foo.span);
        assert_eq!(
            code.instrs,
            vec![Instr::SendLexical {
                depth: 0,
                selector: crate::Symbol::intern("bar"),
                arg_regs: vec![],
                dst: 0,
                call_site: CallSite { call_site_id: 0 },
            }]
        );
        assert_eq!(dst, 0);
    }

    #[test]
    fn a_keyword_implicit_send_and_a_super_send_lower_without_a_receiver_register() {
        let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  at: i put: v = ( ^v )\n  foo = ( at: 1 put: 2. ^super printString )\n) : ()";
        let (foo, bindings) = resolve_method(src, 1);
        let mut lowerer = method_lowerer(&foo, &bindings);
        lowerer.lower_statement(&foo.body.stmts[0]);
        let code = lowerer.finish(foo.span);
        assert!(matches!(
            &code.instrs[2],
            Instr::SendLexical { depth: 0, selector, arg_regs, .. }
                if selector.as_str() == "at:put:" && arg_regs.len() == 2
        ));
        let (foo, bindings) = resolve_method(src, 1);
        let mut lowerer = method_lowerer(&foo, &bindings);
        lowerer.lower_expr(match &foo.body.stmts[1] {
            Stmt::Return(e) => e,
            _ => panic!(),
        });
        let code = lowerer.finish(foo.span);
        assert!(matches!(
            &code.instrs[0],
            Instr::SendSuper { selector, arg_regs, dst: 0, .. }
                if selector.as_str() == "printString" && arg_regs.is_empty()
        ));
    }

    #[test]
    fn a_slot_assignment_lowers_to_a_setter_lexical_send() {
        let src = "Newspeak3\n'T'\nclass X = Object new (\n  | n ::= 0. |\n) (\n  bump = ( n := n + 1 )\n) : ()";
        let (bump, bindings) = resolve_method(src, 0);
        let mut lowerer = method_lowerer(&bump, &bindings);
        lowerer.lower_statement(&bump.body.stmts[0]);
        let code = lowerer.finish(bump.span);
        assert!(matches!(
            code.instrs.last(),
            Some(Instr::SendLexical { depth: 0, selector, arg_regs, .. })
                if selector.as_str() == "n:" && arg_regs.len() == 1
        ));
    }

    #[test]
    fn a_bare_builtin_class_name_lowers_to_load_builtin_class() {
        let src =
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^OrderedCollection )\n) : ()";
        let (foo, bindings) = resolve_method(src, 0);
        let mut lowerer = method_lowerer(&foo, &bindings);
        let dst = lowerer.lower_expr(return_expr(&foo));
        let code = lowerer.finish(foo.span);
        assert_eq!(
            code.instrs,
            vec![Instr::LoadBuiltinClass {
                dst: 0,
                kind: crate::types::BuiltinClass::OrderedCollection
            }]
        );
        assert_eq!(dst, 0);
    }

    #[test]
    fn a_cascade_reuses_one_receiver_register_across_every_message() {
        let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( | c | c := OrderedCollection new. c add: 1; add: 2 )\n) : ()";
        let (foo, bindings) = resolve_method(src, 0);
        let mut lowerer = method_lowerer(&foo, &bindings); // temp `c` is register 0
        lowerer.lower_statement(&foo.body.stmts[0]);
        lowerer.lower_statement(&foo.body.stmts[1]);
        let code = lowerer.finish(foo.span);
        let receiver_regs: Vec<u16> = code
            .instrs
            .iter()
            .filter_map(|i| match i {
                Instr::SendPublic { receiver_reg, .. } => Some(*receiver_reg),
                _ => None,
            })
            .collect();
        // `OrderedCollection new`, then the cascade's two `add:` sends.
        assert_eq!(receiver_regs.len(), 3);
        assert_eq!(
            receiver_regs[1], 0,
            "`c` is register 0, read with no instruction"
        );
        assert_eq!(
            receiver_regs[1], receiver_regs[2],
            "both cascade sends share one receiver register"
        );
        assert_eq!(code.call_sites.len(), 3);
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

    fn lower_whole_method(src: &str, index: usize) -> CompiledCode {
        let (method, bindings) = resolve_method(src, index);
        let mut lowerer = method_lowerer(&method, &bindings);
        for stmt in &method.body.stmts {
            lowerer.lower_statement(stmt);
        }
        lowerer.finish_body(&method.body.stmts);
        lowerer.finish(method.span)
    }

    #[test]
    fn a_method_with_no_explicit_return_implicitly_returns_self() {
        let code = lower_whole_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( 1 + 1 )\n) : ()",
            0,
        );
        assert_eq!(
            &code.instrs[code.instrs.len() - 2..],
            &[Instr::LoadSelf { dst: 3 }, Instr::ImplicitReturn { reg: 3 }]
        );
    }

    #[test]
    fn an_explicit_method_return_is_a_plain_return_with_nothing_appended() {
        let code = lower_whole_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^1 )\n) : ()",
            0,
        );
        assert_eq!(
            code.instrs,
            vec![
                Instr::LoadLiteral {
                    dst: 0,
                    literal: BcLiteral::SmallInt(1)
                },
                Instr::Return { reg: 0 },
            ]
        );
    }

    #[test]
    fn a_block_literal_compiles_to_its_own_code_carried_by_make_closure() {
        let code = lower_whole_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^[ :x | x ] )\n) : ()",
            0,
        );
        let Some(Instr::MakeClosure {
            dst: 0,
            params,
            code: block,
        }) = code.instrs.first()
        else {
            panic!("expected a MakeClosure instruction, got {:?}", code.instrs);
        };
        assert_eq!(params.as_slice(), &[crate::Symbol::intern("x")]);
        assert_eq!(block.kind, CodeKind::Block);
        assert_eq!(block.num_args, 1);
        // `x` is the block's own register 0: read with no instruction, then
        // returned as the block's implicit value.
        assert_eq!(block.instrs, vec![Instr::ImplicitReturn { reg: 0 }]);
        assert_eq!(block.statement_pc, vec![(0, 0..0)]);
    }

    #[test]
    fn a_block_reads_and_writes_enclosing_locals_through_outer_local_instructions() {
        let code = lower_whole_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo: x = ( | t | ^[ t := x ] )\n) : ()",
            0,
        );
        let Some(Instr::MakeClosure { code: block, .. }) = code.instrs.first() else {
            panic!()
        };
        assert_eq!(
            block.instrs,
            vec![
                Instr::LoadOuterLocal {
                    dst: 0,
                    depth: 1,
                    index: 0
                }, // x
                Instr::StoreOuterLocal {
                    src: 0,
                    depth: 1,
                    index: 1
                }, // t
                Instr::ImplicitReturn { reg: 0 },
            ]
        );
    }

    #[test]
    fn a_doubly_nested_block_counts_two_frame_hops() {
        let code = lower_whole_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo: x = ( ^[ [ x ] ] )\n) : ()",
            0,
        );
        let Some(Instr::MakeClosure { code: outer, .. }) = code.instrs.first() else {
            panic!()
        };
        let Some(Instr::MakeClosure { code: inner, .. }) = outer.instrs.first() else {
            panic!()
        };
        assert_eq!(
            inner.instrs[0],
            Instr::LoadOuterLocal {
                dst: 0,
                depth: 2,
                index: 0
            }
        );
    }

    #[test]
    fn a_return_inside_a_block_is_a_non_local_return() {
        let code = lower_whole_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^[ ^1 ] )\n) : ()",
            0,
        );
        let Some(Instr::MakeClosure { code: block, .. }) = code.instrs.first() else {
            panic!()
        };
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
    fn an_empty_block_returns_nil_and_a_multi_statement_block_returns_its_last_value() {
        let code = lower_whole_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( [ ] value. ^[ 1. 2 ] value )\n) : ()",
            0,
        );
        let Some(Instr::MakeClosure { code: empty, .. }) = code.instrs.first() else {
            panic!()
        };
        assert_eq!(
            empty.instrs,
            vec![
                Instr::LoadLiteral {
                    dst: 0,
                    literal: BcLiteral::Nil
                },
                Instr::ImplicitReturn { reg: 0 }
            ]
        );
        let Some(Instr::MakeClosure { code: two, .. }) = code.instrs.get(2) else {
            panic!("{:?}", code.instrs)
        };
        assert_eq!(
            two.instrs,
            vec![
                Instr::LoadLiteral {
                    dst: 0,
                    literal: BcLiteral::SmallInt(1)
                },
                Instr::LoadLiteral {
                    dst: 0,
                    literal: BcLiteral::SmallInt(2)
                },
                Instr::ImplicitReturn { reg: 0 },
            ]
        );
    }

    #[test]
    fn an_empty_inlined_block_yields_nil() {
        // No existing test lowers a literally empty inlined block --
        // `lower_inline_block_body`'s "no statements" path (`result` stays
        // `None`, falling back to `emit_nil`) was previously exercised only
        // indirectly.
        let code = compile_foo("x ifTrue: [] ifFalse: [2]");
        assert!(has(&code, |i| matches!(
            i,
            Instr::LoadLiteral {
                literal: BcLiteral::Nil,
                ..
            }
        )));
        assert!(has(&code, |i| matches!(
            i,
            Instr::LoadLiteral {
                literal: BcLiteral::SmallInt(2),
                ..
            }
        )));
    }

    #[test]
    fn a_dynamic_array_literal_lowers_to_make_array() {
        let (method, bindings) = resolve_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^{ 1. 2. 3 } )\n) : ()",
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings);
        let dst = lowerer.lower_expr(return_expr(&method));
        let code = lowerer.finish(method.span);
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
                Instr::LoadLiteral {
                    dst: 2,
                    literal: BcLiteral::SmallInt(3)
                },
                Instr::MakeArray {
                    dst: 3,
                    elem_regs: vec![0, 1, 2]
                },
            ]
        );
        assert_eq!(dst, 3);
    }

    #[test]
    fn an_initializer_discards_a_top_level_return_and_always_returns_self() {
        let (method, bindings) = resolve_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^1 )\n) : ()",
            0,
        );
        // Reuse `foo`'s `^1` as if it were an initExpr: kind Initializer, 0 args.
        let mut lowerer = Lowerer::new(CodeKind::Initializer, 0, 0, &bindings, vec![]);
        lowerer.lower_statement(&method.body.stmts[0]);
        lowerer.finish_body(&method.body.stmts);
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instrs,
            vec![
                Instr::LoadLiteral {
                    dst: 0,
                    literal: BcLiteral::SmallInt(1)
                },
                Instr::LoadSelf { dst: 1 },
                Instr::ImplicitReturn { reg: 1 },
            ]
        );
    }

    #[test]
    fn a_slot_initializer_is_one_statement_ending_in_init_slot() {
        let (method, bindings) = resolve_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^6 * 7 )\n) : ()",
            0,
        );
        let mut lowerer = Lowerer::new(CodeKind::Initializer, 0, 0, &bindings, vec![]);
        lowerer.lower_slot_init(return_expr(&method), 4);
        let code = lowerer.finish(method.span);
        // `6`, `7`, the `*` send, then the field write: four instructions.
        assert!(matches!(
            code.instrs.last(),
            Some(Instr::InitSlot {
                src: 2,
                field_index: 4
            })
        ));
        assert_eq!(code.statement_pc, vec![(0, 0..4)]);
    }

    /// Compiles `foo: x = ( ^<body> )` and returns the method's code.
    fn compile_foo(body: &str) -> CompiledCode {
        let full =
            format!("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo: x = ( ^{body} )\n) : ()");
        lower_whole_method(&full, 0)
    }

    fn has<F: Fn(&Instr) -> bool>(code: &CompiledCode, pred: F) -> bool {
        code.instrs.iter().any(pred)
    }

    #[test]
    fn if_true_if_false_with_literal_blocks_compiles_to_a_guarded_branch_with_a_fallback_send() {
        let code = compile_foo("x ifTrue: [1] ifFalse: [2]");
        assert!(
            has(&code, |i| matches!(
                i,
                Instr::JumpIfNotBoolean { reg: 0, .. }
            )),
            "{:#?}",
            code.instrs
        );
        assert!(has(&code, |i| matches!(
            i,
            Instr::BranchIfFalse { reg: 0, .. }
        )));
        assert!(has(&code, |i| matches!(
            i,
            Instr::LoadLiteral {
                literal: BcLiteral::SmallInt(1),
                ..
            }
        )));
        assert!(has(&code, |i| matches!(
            i,
            Instr::LoadLiteral {
                literal: BcLiteral::SmallInt(2),
                ..
            }
        )));
        // The fallback path is a real send of the original selector with real closures.
        assert!(has(&code, |i| matches!(
            i,
            Instr::SendPublic { selector, receiver_reg: 0, .. } if selector.as_str() == "ifTrue:ifFalse:"
        )));
        assert_eq!(
            code.instrs
                .iter()
                .filter(|i| matches!(i, Instr::MakeClosure { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn a_non_literal_block_argument_is_an_ordinary_send_with_no_guard() {
        let code = compile_foo("x ifTrue: x ifFalse: [2]");
        assert!(has(&code, |i| matches!(
            i,
            Instr::SendPublic { selector, .. } if selector.as_str() == "ifTrue:ifFalse:"
        )));
        assert!(!has(&code, |i| matches!(i, Instr::JumpIfNotBoolean { .. })));
    }

    #[test]
    fn a_block_with_temporaries_is_not_inlined() {
        let code = compile_foo("x ifTrue: [ | t | t := 1. t ] ifFalse: [2]");
        assert!(!has(&code, |i| matches!(i, Instr::JumpIfNotBoolean { .. })));
        assert!(has(&code, |i| matches!(
            i,
            Instr::MakeClosure { code, .. } if code.num_registers - code.num_args >= 1
        )));
    }

    #[test]
    fn while_true_compiles_to_a_back_edge_jump_and_a_must_be_boolean_fallback() {
        let code = compile_foo("[x] whileTrue: [x]");
        let back_edge = code
            .instrs
            .iter()
            .enumerate()
            .find_map(|(i, instr)| match instr {
                Instr::Jump { target } if (*target as usize) < i => Some(*target),
                _ => None,
            });
        assert!(
            back_edge.is_some(),
            "expected a backward Jump, got {:#?}",
            code.instrs
        );
        assert!(has(&code, |i| matches!(
            i,
            Instr::SendPublic { selector, .. } if selector.as_str() == "mustBeBoolean"
        )));
        assert!(!has(&code, |i| matches!(
            i,
            Instr::SendPublic { selector, .. } if selector.as_str() == "whileTrue:"
        )));
        assert!(
            !has(&code, |i| matches!(i, Instr::MakeClosure { .. })),
            "an inlined loop materializes no closures"
        );
        assert!(has(&code, |i| matches!(i, Instr::BranchIfFalse { .. })));
        let code = compile_foo("[x] whileFalse: [x]");
        assert!(has(&code, |i| matches!(i, Instr::BranchIfTrue { .. })));
    }

    #[test]
    fn a_reference_inside_an_inlined_block_addresses_the_enclosing_frame_directly() {
        // `x` inside the inlined block is `OuterLocal { depth: 1, index: 0 }`
        // to the resolver; at run time it is the current frame's register 0.
        let code = compile_foo("x ifTrue: [x] ifFalse: [nil]");
        assert!(!has(&code, |i| matches!(i, Instr::LoadOuterLocal { .. })));
        assert!(has(&code, |i| matches!(i, Instr::Move { src: 0, .. })));
    }

    #[test]
    fn a_real_block_nested_in_an_inlined_block_counts_one_hop_fewer() {
        // `[ x ]` sits two resolver scopes below the method (inner block ->
        // inlined block -> method) but only one frame below it at run time.
        let code = compile_foo("x ifTrue: [ [ x ] value ] ifFalse: [nil]");
        let nested = code.instrs.iter().find_map(|i| match i {
            Instr::MakeClosure { code, .. } => Some(code.clone()),
            _ => None,
        });
        let nested = nested.expect("the true branch creates a closure");
        assert_eq!(
            nested.instrs[0],
            Instr::LoadOuterLocal {
                dst: 0,
                depth: 1,
                index: 0
            }
        );
    }

    #[test]
    fn a_return_inside_an_inlined_block_follows_the_enclosing_code_kind() {
        let code = compile_foo("x ifTrue: [^1] ifFalse: [^2]");
        assert_eq!(
            code.instrs
                .iter()
                .filter(|i| matches!(i, Instr::Return { .. }))
                .count(),
            3,
            "{:#?}",
            code.instrs
        );
        assert!(!has(&code, |i| matches!(i, Instr::NonLocalReturn { .. })));
        // Inside a block body the same `^` is a non-local return.
        let code = compile_foo("[ x ifTrue: [^1] ifFalse: [^2] ]");
        let Some(Instr::MakeClosure { code: block, .. }) = code.instrs.first() else {
            panic!()
        };
        assert!(block
            .instrs
            .iter()
            .any(|i| matches!(i, Instr::NonLocalReturn { .. })));
    }

    #[test]
    fn inlined_statements_do_not_clobber_live_registers_of_the_enclosing_statement() {
        // `x foo: (x bar) with: (x ifTrue: [x baz. 1] ifFalse: [2])`: the
        // first argument's register must survive the inlined `x baz` statement.
        let code = compile_foo("x foo: (x bar) with: (x ifTrue: [x baz. 1] ifFalse: [2])");
        let bar_dst = code
            .instrs
            .iter()
            .find_map(|i| match i {
                Instr::SendPublic { selector, dst, .. } if selector.as_str() == "bar" => Some(*dst),
                _ => None,
            })
            .expect("a bar send");
        let baz_dst = code
            .instrs
            .iter()
            .find_map(|i| match i {
                Instr::SendPublic { selector, dst, .. } if selector.as_str() == "baz" => Some(*dst),
                _ => None,
            })
            .expect("a baz send");
        assert_ne!(bar_dst, baz_dst);
        let Some(Instr::SendPublic {
            selector, arg_regs, ..
        }) = code.instrs.iter().rev().find(
            |i| matches!(i, Instr::SendPublic { selector, .. } if selector.as_str() == "foo:with:"),
        )
        else {
            panic!("expected a foo:with: send")
        };
        assert_eq!(selector.as_str(), "foo:with:");
        assert_eq!(arg_regs[0], bar_dst);
    }

    #[test]
    fn and_or_coerce_the_block_value_and_short_circuit() {
        let code = compile_foo("x and: [x]");
        assert!(has(&code, |i| matches!(
            i,
            Instr::LoadLiteral {
                literal: BcLiteral::Bool(true),
                ..
            }
        )));
        assert!(has(&code, |i| matches!(
            i,
            Instr::LoadLiteral {
                literal: BcLiteral::Bool(false),
                ..
            }
        )));
        assert!(has(&code, |i| matches!(
            i,
            Instr::SendPublic { selector, .. } if selector.as_str() == "and:"
        )));
        let code = compile_foo("x or: [x]");
        assert!(has(&code, |i| matches!(
            i,
            Instr::SendPublic { selector, .. } if selector.as_str() == "or:"
        )));
    }

    #[test]
    fn mutually_exclusive_branch_arms_reuse_registers_instead_of_stacking() {
        // Each arm needs several live registers on its own (a chain of two
        // binary sends holding both operands alive), but the arms never run
        // together -- register count must stay bounded by the costliest
        // single arm, not grow with the sum of all three arms' code size.
        // Before the `emit_guarded_branch` scratch-allocator reset, this
        // body compiled to 15 registers; the arms now share scratch space
        // and it compiles to 7 (`self` r0, `x` r1, guard+dst r2-r3, and the
        // costliest arm's own 3 scratch registers r4-r6).
        let code = compile_foo("x ifTrue: [(x + 1) + (x + 2)] ifFalse: [(x + 3) + (x + 4)]");
        assert_eq!(code.num_registers, 7, "{:#?}", code.instrs);
    }

    #[test]
    fn inlined_statements_are_inside_the_enclosing_statements_pc_range() {
        let code = compile_foo("x ifTrue: [x foo. x bar] ifFalse: [nil]");
        assert_eq!(
            code.statement_pc.len(),
            1,
            "the inlined statements are part of statement 0"
        );
        assert_eq!(code.statement_pc[0].1, 0..code.instrs.len());
    }
}
