//! The bytecode instruction set and compiled-code data shapes. Pure data --
//! see `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md`'s
//! `bytecode.rs` section. `Instr` is a plain Rust enum, `match`-dispatched
//! by `vm.rs`; "architecture-neutral, versioned" describes `image::encode`'s
//! wire format for a `CompiledCode`, not this in-memory type.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use ns_syntax::{ast, Span};

use crate::types::{Binding, BuiltinClass, ClassRef};
use crate::Symbol;

/// A compiled instruction-set literal -- a lowered form of `ast::Literal`
/// that keeps only what execution needs. Int/float text is parsed once at
/// compile time; `Str`/`Sym`/`Char`/`Array` still allocate a fresh heap
/// object each time they execute (matching `eval::eval_literal` -- a literal
/// string is a fresh `Object` on every evaluation in Milestone 1 too).
///
/// `MalformedInt` carries the raw text of an integer literal the lexer
/// accepted but `interpreter::eval::parse_int_literal` rejects (out of
/// `i64` range, a fractional radix literal). That is a Newspeak-program
/// error M1 raises *when the literal is evaluated*; compilation cannot
/// raise a `NewspeakError`, so the VM re-runs `parse_int_literal` when the
/// instruction executes and raises the identical error then.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Nil,
    Bool(bool),
    SmallInt(i64),
    Float(f64),
    Str(String),
    Sym(Symbol),
    Char(char),
    /// Element literals, recursively -- mirrors `ast::Literal::Array`.
    Array(Vec<Literal>),
    MalformedInt(String),
}

/// One send instruction's stable identity -- the instruction's own index
/// into `CompiledCode.instrs`. Reserved for Milestone 3's inline caches;
/// inert in Milestone 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallSite {
    pub call_site_id: u16,
}

/// Every register operand is a `u16` index into the frame's unified
/// args-then-temps register file; every jump target is an instruction
/// index into the owning `CompiledCode.instrs`.
#[derive(Debug, Clone, PartialEq)]
pub enum Instr {
    LoadLiteral {
        dst: u16,
        literal: Literal,
    },
    LoadSelf {
        dst: u16,
    },
    /// M1's "bare builtin class name has no `Binding`" convention
    /// (`eval::eval_ident`'s fallback), resolved at compile time.
    LoadBuiltinClass {
        dst: u16,
        kind: BuiltinClass,
    },
    Move {
        dst: u16,
        src: u16,
    },
    /// `Binding::Local` write. A read needs no instruction -- the local's
    /// register *is* its value.
    StoreLocal {
        src: u16,
        slot: u16,
    },
    /// `Binding::OuterLocal` read/write through the `captured_frame` chain.
    LoadOuterLocal {
        dst: u16,
        depth: u16,
        index: u16,
    },
    StoreOuterLocal {
        src: u16,
        depth: u16,
        index: u16,
    },
    /// `Binding::LexicalSend`: kth-enclosing-object + private/`lookupProtected`.
    /// No receiver register -- the receiver is `self` by construction.
    SendLexical {
        depth: u16,
        selector: Symbol,
        arg_regs: Vec<u16>,
        dst: u16,
        call_site: CallSite,
    },
    /// An ordinary, explicit-receiver send (`lookupPublic`).
    SendPublic {
        selector: Symbol,
        receiver_reg: u16,
        arg_regs: Vec<u16>,
        dst: u16,
        call_site: CallSite,
    },
    /// `Binding::SuperSend`: receiver is `self`, lookup starts at the
    /// current activation's `current_class`'s superclass.
    SendSuper {
        selector: Symbol,
        arg_regs: Vec<u16>,
        dst: u16,
        call_site: CallSite,
    },
    /// Builds a closure over the *compiled* block body. `captured_frame`,
    /// `home`, and `bindings` are runtime values the VM supplies from its
    /// current frame when the instruction executes.
    MakeClosure {
        dst: u16,
        params: Rc<Vec<Symbol>>,
        code: Rc<CompiledCode>,
    },
    MakeArray {
        dst: u16,
        elem_regs: Vec<u16>,
    },
    /// Writes `self`'s instance field `field_index` -- emitted only by
    /// instance-initializer compilation for a slot's own initializer
    /// expression. The one layout-dependent instruction (spec: Milestone
    /// 3's class-shape-change flush must invalidate compiled initializers).
    InitSlot {
        src: u16,
        field_index: u32,
    },
    /// A method or initializer body's `^`.
    Return {
        reg: u16,
    },
    /// A body that fell off its end: a method returns `self`, a block its
    /// last expression's value. Treated by the VM exactly like `Return`;
    /// kept distinct so `disasm` output stays legible.
    ImplicitReturn {
        reg: u16,
    },
    /// A `^` inside a block body -- unwinds to the activation's `home` via
    /// the shared `Interpreter::raise_return`.
    NonLocalReturn {
        reg: u16,
    },
    /// Unconditional jump. Emitted only by control-flow inlining.
    Jump {
        target: u32,
    },
    /// Control-flow inlining's runtime guard: jumps if `reg` does not hold
    /// a `Value::Bool` at all.
    JumpIfNotBoolean {
        reg: u16,
        target: u32,
    },
    /// Jumps if `reg` is exactly `Value::Bool(false)`; falls through for
    /// anything else (a `JumpIfNotBoolean` on the same register always
    /// precedes it in inlined code).
    BranchIfFalse {
        reg: u16,
        target: u32,
    },
    /// Symmetric to `BranchIfFalse`; used for `whileFalse:`'s exit.
    BranchIfTrue {
        reg: u16,
        target: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeKind {
    Method,
    Block,
    Initializer,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledCode {
    pub kind: CodeKind,
    pub instrs: Vec<Instr>,
    pub num_args: u16,
    /// Total register count: `num_args` + named temps + compiler-allocated
    /// scratch registers. The engine hooks size `Frame.temps` to
    /// `num_registers - num_args`.
    pub num_registers: u16,
    /// Maps a statement index (a method's/block's `CodeBody.stmts` index;
    /// for an initializer, each slot initializer then each `initExpr`, in
    /// that order) to the half-open instruction range it compiled to --
    /// the safepoint boundary and the M1-compatible "statement `pc`" view.
    pub statement_pc: Vec<(usize, Range<usize>)>,
    /// One entry per `Send*` instruction, in instruction order. Inert.
    pub call_sites: Vec<CallSite>,
    pub source: Span,
}

/// The bytecode engine's per-module cache, installed on the `Interpreter`
/// as `Engine::Bytecode(module)`. Keyed by `Rc` pointer identity of the
/// *loaded* `MethodDecl`/`ClassRef` -- the exact `Rc`s `dispatch.rs` hands
/// back -- so no name-based index can drift and no two same-named methods
/// on different classes can collide.
#[derive(Debug)]
pub struct CompiledModule {
    pub methods: HashMap<usize, Rc<CompiledCode>>,
    pub initializers: HashMap<usize, Rc<CompiledCode>>,
    /// The module's resolver output, carried so the hooks can pass it to
    /// the shared orchestration's `bindings` parameters.
    pub bindings: Rc<HashMap<Span, Binding>>,
}

pub fn method_key(method: &Rc<ast::MethodDecl>) -> usize {
    Rc::as_ptr(method) as *const () as usize
}

pub fn class_key(class: &ClassRef) -> usize {
    Rc::as_ptr(class) as *const () as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_code_construction_smoke_test() {
        let code = CompiledCode {
            kind: CodeKind::Method,
            instrs: vec![
                Instr::LoadLiteral {
                    dst: 1,
                    literal: Literal::SmallInt(42),
                },
                Instr::Return { reg: 1 },
            ],
            num_args: 1,
            num_registers: 2,
            statement_pc: vec![(0, 0..2)],
            call_sites: vec![],
            source: Span::new(0, 10),
        };
        assert_eq!(code.instrs.len(), 2);
        assert_eq!(code.num_registers - code.num_args, 1);
        assert_eq!(code, code.clone());
    }

    #[test]
    fn call_site_id_is_copy_and_comparable() {
        let a = CallSite { call_site_id: 3 };
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn send_instr_variants_carry_a_call_site() {
        let send = Instr::SendPublic {
            selector: Symbol::intern("foo:"),
            receiver_reg: 0,
            arg_regs: vec![1],
            dst: 2,
            call_site: CallSite { call_site_id: 0 },
        };
        assert!(matches!(
            send,
            Instr::SendPublic {
                call_site: CallSite { call_site_id: 0 },
                ..
            }
        ));
    }

    #[test]
    fn make_closure_carries_compiled_block_code() {
        let block = Rc::new(CompiledCode {
            kind: CodeKind::Block,
            instrs: vec![Instr::ImplicitReturn { reg: 0 }],
            num_args: 1,
            num_registers: 1,
            statement_pc: vec![(0, 0..1)],
            call_sites: vec![],
            source: Span::new(0, 5),
        });
        let make = Instr::MakeClosure {
            dst: 0,
            params: Rc::new(vec![Symbol::intern("x")]),
            code: block,
        };
        let Instr::MakeClosure { code, .. } = &make else {
            panic!()
        };
        assert_eq!(code.kind, CodeKind::Block);
    }

    #[test]
    fn method_key_is_rc_pointer_identity() {
        let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^1 )\n) : ()";
        let module = ns_syntax::parse(src).expect("parse");
        let method = Rc::new(module.root_class.instance_side.methods[0].clone());
        let same = method.clone();
        let other = Rc::new(module.root_class.instance_side.methods[0].clone());
        assert_eq!(method_key(&method), method_key(&same));
        assert_ne!(method_key(&method), method_key(&other));
    }
}
