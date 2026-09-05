//! `Expr`/`Stmt` -> `Instr` lowering, register allocation, and the scope
//! stack that maps resolver depths onto runtime frame hops. See
//! `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md`'s
//! `bytecode.rs` and "Control-flow inlining" sections.
//!
//! `Lowerer` has no caller outside `#[cfg(test)]` yet -- Task 5 wraps it
//! into `compile_method`/`compile_initializer`/`compile_module`. Until then
//! the non-test build sees every `pub(crate)` item here as unreachable;
//! `dead_code` is silenced for that reason alone and should come back on
//! its own once Task 5 lands the call sites.
#![allow(dead_code)]

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use ns_syntax::ast::{self, Expr, Stmt};
use ns_syntax::Span;

use crate::bytecode::{CallSite, CodeKind, CompiledCode, Instr, Literal as BcLiteral};
use crate::interpreter::eval::parse_int_literal;
use crate::types::Binding;

/// One entry of the compile-time scope stack. The resolver counts every
/// block literal as one scope; at run time only `Frame` scopes cost a
/// `captured_frame` hop -- an `Inlined` scope (Task 11) has no frame.
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
    /// until Task 11 pushes `Inlined` entries above it.
    scopes: Vec<ScopeKind>,
    instrs: Vec<Instr>,
    base: u16,
    next_scratch: u16,
    max_reg: u16,
    statement_index: usize,
    statement_pc: Vec<(usize, Range<usize>)>,
    call_sites: Vec<CallSite>,
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
        match stmt {
            Stmt::Expr(e) => {
                self.lower_expr(e);
            }
            Stmt::Return(_) => unimplemented!("Stmt::Return is lowered starting in Task 4"),
        }
        let end = self.instrs.len();
        self.statement_pc.push((self.statement_index, start..end));
        self.statement_index += 1;
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
            Expr::Send(_) | Expr::Cascade(_) => {
                unimplemented!("sends/cascades are lowered starting in Task 3")
            }
            Expr::Block(_) => unimplemented!("blocks are lowered starting in Task 4"),
            Expr::DynamicArray { .. } => unimplemented!("arrays are lowered starting in Task 4"),
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
            Some(Binding::LexicalSend { .. }) | Some(Binding::SuperSend) | None => {
                unimplemented!("implicit/lexical/builtin-name reads are lowered starting in Task 3")
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
            Some(Binding::LexicalSend { .. }) | Some(Binding::SuperSend) | None => {
                unimplemented!("setter-send assignment is lowered starting in Task 3")
            }
        }
        // An assignment's own value is `value`'s value (runtime spec's
        // "Setter sends" note) -- already sitting in register `v`.
        v
    }

    pub(crate) fn finish(self, source: Span) -> CompiledCode {
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
}
