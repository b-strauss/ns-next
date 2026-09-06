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
///
/// `UnresolvedIdent` is the same deferral for the other program-level
/// error a *resolved* module can still carry: an identifier with no
/// binding that names no builtin class -- in practice a bare `super`,
/// which `resolver::resolve_ident` leaves unbound and the lexer does not
/// reserve. Neither variant is a lowered `ast::Literal`; both are payloads
/// of a `LoadLiteral` that raises instead of producing a value.
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
    UnresolvedIdent(String),
}

/// One send instruction's stable identity -- the instruction's own index
/// into `CompiledCode.instrs`. Reserved for Milestone 4's inline caches;
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

/// A human-readable listing of `code` for `ns-next disasm`. Nested block
/// code is printed indented under its `MakeClosure`. Pure formatting.
pub fn disassemble(code: &CompiledCode, name: &str) -> String {
    let mut out = String::new();
    write_code(&mut out, code, name, 0);
    out
}

fn write_code(out: &mut String, code: &CompiledCode, name: &str, indent: usize) {
    use std::fmt::Write;
    let pad = " ".repeat(indent);
    let kind = match code.kind {
        CodeKind::Method => "method",
        CodeKind::Block => "block",
        CodeKind::Initializer => "initializer",
    };
    writeln!(
        out,
        "{pad}{kind} {name} (args {}, registers {})",
        code.num_args, code.num_registers
    )
    .expect("writing to a String cannot fail");
    for (idx, instr) in code.instrs.iter().enumerate() {
        let stmt = code
            .statement_pc
            .iter()
            .find(|(_, range)| range.contains(&idx))
            .map(|(s, _)| format!("stmt {s}"))
            .unwrap_or_else(|| "-".to_string());
        writeln!(out, "{pad}{idx:4}  {stmt:>7}  {}", format_instr(instr))
            .expect("writing to a String cannot fail");
        if let Instr::MakeClosure { code: block, .. } = instr {
            write_code(out, block, "block", indent + 8);
        }
    }
}

fn regs(regs: &[u16]) -> String {
    regs.iter()
        .map(|r| format!("r{r}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_instr(instr: &Instr) -> String {
    match instr {
        Instr::LoadLiteral { dst, literal } => {
            format!("LoadLiteral r{dst}, {}", format_literal(literal))
        }
        Instr::LoadSelf { dst } => format!("LoadSelf r{dst}"),
        Instr::LoadBuiltinClass { dst, kind } => format!("LoadBuiltinClass r{dst}, {kind:?}"),
        Instr::Move { dst, src } => format!("Move r{dst}, r{src}"),
        Instr::StoreLocal { src, slot } => format!("StoreLocal r{slot} <- r{src}"),
        Instr::LoadOuterLocal { dst, depth, index } => {
            format!("LoadOuterLocal r{dst}, depth {depth} index {index}")
        }
        Instr::StoreOuterLocal { src, depth, index } => {
            format!("StoreOuterLocal depth {depth} index {index} <- r{src}")
        }
        Instr::SendLexical {
            depth,
            selector,
            arg_regs,
            dst,
            call_site,
        } => format!(
            "SendLexical r{dst} <- depth {depth} #{selector} ({}) @{}",
            regs(arg_regs),
            call_site.call_site_id
        ),
        Instr::SendPublic {
            selector,
            receiver_reg,
            arg_regs,
            dst,
            call_site,
        } => format!(
            "SendPublic r{dst} <- r{receiver_reg} #{selector} ({}) @{}",
            regs(arg_regs),
            call_site.call_site_id
        ),
        Instr::SendSuper {
            selector,
            arg_regs,
            dst,
            call_site,
        } => format!(
            "SendSuper r{dst} <- #{selector} ({}) @{}",
            regs(arg_regs),
            call_site.call_site_id
        ),
        Instr::MakeClosure { dst, params, .. } => format!(
            "MakeClosure r{dst}, [{}] ->",
            params
                .iter()
                .map(|p| format!(":{p}"))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Instr::MakeArray { dst, elem_regs } => format!("MakeArray r{dst}, ({})", regs(elem_regs)),
        Instr::InitSlot { src, field_index } => format!("InitSlot field {field_index} <- r{src}"),
        Instr::Return { reg } => format!("Return r{reg}"),
        Instr::ImplicitReturn { reg } => format!("ImplicitReturn r{reg}"),
        Instr::NonLocalReturn { reg } => format!("NonLocalReturn r{reg}"),
        Instr::Jump { target } => format!("Jump -> {target}"),
        Instr::JumpIfNotBoolean { reg, target } => format!("JumpIfNotBoolean r{reg} -> {target}"),
        Instr::BranchIfFalse { reg, target } => format!("BranchIfFalse r{reg} -> {target}"),
        Instr::BranchIfTrue { reg, target } => format!("BranchIfTrue r{reg} -> {target}"),
    }
}

fn format_literal(literal: &Literal) -> String {
    match literal {
        Literal::Nil => "nil".to_string(),
        Literal::Bool(b) => b.to_string(),
        Literal::SmallInt(n) => n.to_string(),
        Literal::Float(f) => format!("{f:?}"),
        Literal::Str(s) => format!("'{s}'"),
        Literal::Sym(s) => format!("#{s}"),
        Literal::Char(c) => format!("\"{c}\""),
        Literal::Array(elements) => format!(
            "#({})",
            elements
                .iter()
                .map(format_literal)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Literal::MalformedInt(text) => format!("malformed-int '{text}'"),
        Literal::UnresolvedIdent(name) => format!("unresolved-ident '{name}'"),
    }
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

    #[test]
    fn disassemble_lists_instructions_with_statement_labels_and_nested_blocks() {
        let block = Rc::new(CompiledCode {
            kind: CodeKind::Block,
            instrs: vec![
                Instr::LoadOuterLocal {
                    dst: 0,
                    depth: 1,
                    index: 0,
                },
                Instr::NonLocalReturn { reg: 0 },
            ],
            num_args: 0,
            num_registers: 1,
            statement_pc: vec![(0, 0..2)],
            call_sites: vec![],
            source: Span::new(0, 5),
        });
        let code = CompiledCode {
            kind: CodeKind::Method,
            instrs: vec![
                Instr::MakeClosure {
                    dst: 1,
                    params: Rc::new(vec![]),
                    code: block,
                },
                Instr::SendPublic {
                    selector: Symbol::intern("value"),
                    receiver_reg: 1,
                    arg_regs: vec![],
                    dst: 2,
                    call_site: CallSite { call_site_id: 1 },
                },
                Instr::LoadSelf { dst: 3 },
                Instr::ImplicitReturn { reg: 3 },
            ],
            num_args: 1,
            num_registers: 4,
            statement_pc: vec![(0, 0..2)],
            call_sites: vec![CallSite { call_site_id: 1 }],
            source: Span::new(0, 10),
        };
        let text = disassemble(&code, "foo:");
        let expected = "\
method foo: (args 1, registers 4)
   0   stmt 0  MakeClosure r1, [] ->
        block block (args 0, registers 1)
           0   stmt 0  LoadOuterLocal r0, depth 1 index 0
           1   stmt 0  NonLocalReturn r0
   1   stmt 0  SendPublic r2 <- r1 #value () @1
   2        -  LoadSelf r3
   3        -  ImplicitReturn r3
";
        assert_eq!(text, expected);
    }

    #[test]
    fn format_literal_covers_every_variant() {
        let lits = vec![
            Literal::Nil,
            Literal::Bool(true),
            Literal::SmallInt(-3),
            Literal::Float(2.5),
            Literal::Str("s".to_string()),
            Literal::Sym(Symbol::intern("sym")),
            Literal::Char('c'),
            Literal::Array(vec![Literal::SmallInt(1), Literal::Nil]),
            Literal::MalformedInt("16r1.8".to_string()),
        ];
        let rendered: Vec<String> = lits.iter().map(format_literal).collect();
        assert_eq!(
            rendered,
            vec![
                "nil",
                "true",
                "-3",
                "2.5",
                "'s'",
                "#sym",
                "\"c\"",
                "#(1 nil)",
                "malformed-int '16r1.8'"
            ]
        );
    }
}
