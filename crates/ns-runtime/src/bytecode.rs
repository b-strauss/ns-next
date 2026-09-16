//! The bytecode instruction set and compiled-code data shapes. Pure data -- see
//! `docs/design/compiler.md`'s "The instruction set" section. `Instruction` is
//! a plain Rust enum, `match`-dispatched by `vm.rs`; "architecture-neutral,
//! versioned" describes `image::encode`'s wire format for a `CompiledCode`, not
//! this in-memory type.

use std::ops::Range;
use std::rc::Rc;

use ns_syntax::Span;

use crate::types::BuiltinClass;
use crate::Symbol;

/// A compiled instruction-set literal -- a lowered form of `ast::Literal` that
/// keeps only what execution needs. Int/float text is parsed once at compile
/// time; `String`/`Symbol`/`Character`/`Array` still allocate a fresh heap
/// object each time they execute (a literal string is a fresh `Object` on every
/// evaluation).
///
/// `MalformedInteger` carries the raw text of an integer literal the lexer
/// accepted but `interpreter::literals::parse_integer_literal` rejects (out of
/// `i64` range, a fractional radix literal). That is a Newspeak-program
/// error the language raises *when the literal is evaluated*; compilation cannot
/// raise a `NewspeakError`, so the VM re-runs `parse_integer_literal` when the
/// instruction executes and raises the identical error then.
///
/// `UnresolvedIdentifier` is the same deferral for the other program-level
/// error a *resolved* module can still carry: an identifier with no binding
/// that names no builtin class -- a bare `super`, which
/// `resolver::resolve_identifier` leaves unbound and the lexer does not
/// reserve, or a `super`-prefixed superclass-clause receiver (see
/// `interpreter::literals::unresolved_identifier_error`). Neither variant is a
/// lowered `ast::Literal`; both are payloads of a `LoadLiteral` that raises
/// instead of producing a value.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Nil,
    Boolean(bool),
    SmallInteger(i64),
    Float(f64),
    String(String),
    Symbol(Symbol),
    Character(char),
    /// Element literals, recursively -- mirrors `ast::Literal::Array`.
    Array(Vec<Literal>),
    MalformedInteger(String),
    UnresolvedIdentifier(String),
}

/// One send's identity within its code family (`docs/design/dispatch.md`,
/// "Code families"): its number among every send of one root body and the
/// blocks nested in it, dense from 0. `UNCACHED` is what the reference
/// lowering emits, so every send it compiles runs the full lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallSite {
    pub number: u32,
}

impl CallSite {
    pub const UNCACHED: CallSite = CallSite { number: u32::MAX };
}

/// Every register operand is a `u16` index into the frame's unified
/// arguments-then-temporaries register file; every jump target is an instruction
/// index into the owning `CompiledCode.instructions`.
#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    LoadLiteral {
        destination: u16,
        literal: Literal,
    },
    LoadSelf {
        destination: u16,
    },
    /// A bare builtin class name, which the resolver leaves without a
    /// `Binding`, resolved at compile time.
    LoadBuiltinClass {
        destination: u16,
        kind: BuiltinClass,
    },
    Move {
        destination: u16,
        source: u16,
    },
    /// `Binding::Local` write. A read needs no instruction -- the local's
    /// register *is* its value.
    StoreLocal {
        source: u16,
        slot: u16,
    },
    /// `Binding::OuterLocal` read/write through the `captured_frame` chain.
    LoadOuterLocal {
        destination: u16,
        depth: u16,
        index: u16,
    },
    StoreOuterLocal {
        source: u16,
        depth: u16,
        index: u16,
    },
    /// `Binding::LexicalSend`: kth-enclosing-object + private/`lookupProtected`.
    /// No receiver register -- the receiver is `self` by construction.
    SendLexical {
        depth: u16,
        selector: Symbol,
        argument_registers: Vec<u16>,
        destination: u16,
        call_site: CallSite,
    },
    /// An ordinary, explicit-receiver send (`lookupPublic`).
    SendPublic {
        selector: Symbol,
        receiver_register: u16,
        argument_registers: Vec<u16>,
        destination: u16,
        call_site: CallSite,
    },
    /// An ordinary explicit-receiver send, exactly `SendPublic`, emitted only
    /// by the reference lowering and only for the sends the optimized lowering
    /// inlines (`control_flow::recognize`). The mark is for the depth count: a
    /// block a builtin control-flow native activates for it is not counted,
    /// because the optimized lowering never activates that block at all
    /// (`docs/design/compiler.md`, "How the limit counts").
    SendInlinable {
        selector: Symbol,
        receiver_register: u16,
        argument_registers: Vec<u16>,
        destination: u16,
        call_site: CallSite,
    },
    /// `Binding::SuperSend`: receiver is `self`, lookup starts at the
    /// current activation's `current_class`'s superclass.
    SendSuper {
        selector: Symbol,
        argument_registers: Vec<u16>,
        destination: u16,
        call_site: CallSite,
    },
    /// Builds a closure over the *compiled* block body. `captured_frame` and
    /// `home` are runtime values the VM supplies from its current frame
    /// when the instruction executes.
    MakeClosure {
        destination: u16,
        parameters: Rc<Vec<Symbol>>,
        code: Rc<CompiledCode>,
    },
    MakeArray {
        destination: u16,
        element_registers: Vec<u16>,
    },
    /// Writes `self`'s instance field at `frame.slot_base + own_index` --
    /// emitted only by instance-initializer compilation for a slot's own
    /// initializer expression. `own_index` is the slot's position among the
    /// *mixin's own* slots (declaration order), never an absolute field
    /// index: the compiled initializer is shared by every application of
    /// its mixin (it lives on the mixin, not on a class object -- §6.2),
    /// and different applications legitimately have
    /// different absolute offsets for the same mixin's own slots depending
    /// on how many slots their own superclass contributes. The VM
    /// resolves the absolute index at run time via `frame.slot_base`, which
    /// is fixed when that level's activation is pushed, from the slot base
    /// `Mutator::initializer_body` finds: that level's own
    /// `MixinApplication.own_slot_base` -- one frame per level, so a frame's
    /// base never describes a level other than its own. Still the one
    /// layout-dependent instruction (a class-shape-change flush must
    /// invalidate compiled initializers: `docs/design/liveness-and-reflection.md`,
    /// "Caches and their dependency keys") -- layout-dependent on the
    /// mixin's own slot order, not on the whole hierarchy's.
    InitializeSlot {
        source: u16,
        own_index: u32,
    },
    /// A method or initializer body's `^`.
    Return {
        register: u16,
    },
    /// A body that fell off its end: a method returns `self`, a block its
    /// last expression's value. Treated by the VM exactly like `Return`;
    /// kept distinct so `disasm` output stays legible.
    ImplicitReturn {
        register: u16,
    },
    /// A `^` inside a block body -- unwinds to the activation's `home` via
    /// the shared `Mutator::raise_return`.
    NonLocalReturn {
        register: u16,
    },
    /// Unconditional jump. Emitted only by control-flow inlining.
    Jump {
        target: u32,
    },
    /// Control-flow inlining's runtime guard: jumps if `register` does not hold
    /// a `Value::Boolean` at all.
    JumpIfNotBoolean {
        register: u16,
        target: u32,
    },
    /// Jumps if `register` is exactly `Value::Boolean(false)`; falls through
    /// for anything else (a `JumpIfNotBoolean` on the same register always
    /// precedes it in inlined code).
    BranchIfFalse {
        register: u16,
        target: u32,
    },
    /// Symmetric to `BranchIfFalse`; used for `whileFalse:`'s exit.
    BranchIfTrue {
        register: u16,
        target: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeKind {
    Method,
    Block,
    Initializer,
    /// A class declaration's superclass clause: the expression
    /// `newspeak-spec.pdf` §6.1 calls `ec`, compiled as a zero-argument body
    /// that evaluates it and returns the class object it produced. It is a
    /// code kind of its own rather than a `Method` with no declaration
    /// because the frame it runs in belongs to the *enclosing* class and
    /// carries no factory arguments -- `image::decode` reconciles a decoded
    /// body against what it is attached to by kind, so the two must not be
    /// confusable there.
    SuperclassClause,
    /// The arguments of a class declaration's superclass clause's deferred
    /// factory message (`7` in `= Base withValue: 7`), compiled as a body
    /// whose `argument_count` is the *declaring* class's own factory parameter
    /// count and whose value is the argument list as a dynamic array.
    /// `newspeak-spec.pdf` §6.2 evaluates that message fresh on every
    /// instantiation, in that instantiation's own initializer context, so
    /// -- unlike `SuperclassClause`, which runs once per enclosing object
    /// and on the enclosing object -- this body sees the subclass's own
    /// factory arguments. A kind of its own for the same reason
    /// `SuperclassClause` is one: `image::decode` tells a decoded body's
    /// purpose by its kind, so the two must not be confusable there.
    SuperclassMessage,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledCode {
    pub kind: CodeKind,
    pub instructions: Vec<Instruction>,
    pub argument_count: u16,
    /// Total register count: `argument_count` + named temporaries +
    /// compiler-allocated scratch registers. An activation is pushed with
    /// `register_count - argument_count`.
    pub register_count: u16,
    /// Maps a statement index (a method's/block's `CodeBody.statements` index;
    /// for an initializer, each slot initializer then each `initExpr`, in that
    /// order) to the half-open instruction range it compiled to -- the
    /// safepoint boundary, and how an activation's instruction
    /// `program_counter` maps back to the statement it is in.
    pub statement_program_counter: Vec<(usize, Range<usize>)>,
    /// One past the highest call-site number among this body's own sends
    /// and its nested blocks' `family_size`s; 0 when there is none.
    pub family_size: u32,
    pub source: Span,
}

impl Instruction {
    /// A send's call site; `None` for every other instruction.
    pub fn call_site(&self) -> Option<CallSite> {
        match self {
            Instruction::SendLexical { call_site, .. }
            | Instruction::SendPublic { call_site, .. }
            | Instruction::SendInlinable { call_site, .. }
            | Instruction::SendSuper { call_site, .. } => Some(*call_site),
            _ => None,
        }
    }
}

/// One past the highest call-site number among `instructions`' sends and
/// the `family_size` of every block they create; 0 when there is none.
/// `UNCACHED` sends are not numbered. The compiler and the decoder both
/// compute it here, so the two cannot disagree about a body.
pub(crate) fn family_size_of(instructions: &[Instruction]) -> u32 {
    instructions
        .iter()
        .map(|instruction| match instruction {
            Instruction::MakeClosure { code, .. } => code.family_size,
            other => match other.call_site() {
                Some(site) if site != CallSite::UNCACHED => site.number + 1,
                _ => 0,
            },
        })
        .max()
        .unwrap_or(0)
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
        CodeKind::SuperclassClause => "superclass clause",
        CodeKind::SuperclassMessage => "superclass clause message",
    };
    writeln!(
        out,
        "{pad}{kind} {name} (args {}, registers {})",
        code.argument_count, code.register_count
    )
    .expect("writing to a String cannot fail");
    for (index, instruction) in code.instructions.iter().enumerate() {
        let statement = code
            .statement_program_counter
            .iter()
            .find(|(_, range)| range.contains(&index))
            .map(|(statement_index, _)| format!("stmt {statement_index}"))
            .unwrap_or_else(|| "-".to_string());
        writeln!(
            out,
            "{pad}{index:4}  {statement:>7}  {}",
            format_instruction(instruction)
        )
        .expect("writing to a String cannot fail");
        if let Instruction::MakeClosure { code: block, .. } = instruction {
            write_code(out, block, "block", indent + 8);
        }
    }
}

fn registers(registers: &[u16]) -> String {
    registers
        .iter()
        .map(|register| format!("r{register}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_instruction(instruction: &Instruction) -> String {
    match instruction {
        Instruction::LoadLiteral {
            destination,
            literal,
        } => {
            format!("LoadLiteral r{destination}, {}", format_literal(literal))
        }
        Instruction::LoadSelf { destination } => format!("LoadSelf r{destination}"),
        Instruction::LoadBuiltinClass { destination, kind } => {
            format!("LoadBuiltinClass r{destination}, {kind:?}")
        }
        Instruction::Move {
            destination,
            source,
        } => format!("Move r{destination}, r{source}"),
        Instruction::StoreLocal { source, slot } => format!("StoreLocal r{slot} <- r{source}"),
        Instruction::LoadOuterLocal {
            destination,
            depth,
            index,
        } => {
            format!("LoadOuterLocal r{destination}, depth {depth} index {index}")
        }
        Instruction::StoreOuterLocal {
            source,
            depth,
            index,
        } => {
            format!("StoreOuterLocal depth {depth} index {index} <- r{source}")
        }
        Instruction::SendLexical {
            depth,
            selector,
            argument_registers,
            destination,
            call_site,
        } => format!(
            "SendLexical r{destination} <- depth {depth} #{selector} ({}) {}",
            registers(argument_registers),
            format_call_site(*call_site)
        ),
        Instruction::SendPublic {
            selector,
            receiver_register,
            argument_registers,
            destination,
            call_site,
        } => format!(
            "SendPublic r{destination} <- r{receiver_register} #{selector} ({}) {}",
            registers(argument_registers),
            format_call_site(*call_site)
        ),
        Instruction::SendInlinable {
            selector,
            receiver_register,
            argument_registers,
            destination,
            call_site,
        } => format!(
            "SendInlinable r{destination} <- r{receiver_register} #{selector} ({}) {}",
            registers(argument_registers),
            format_call_site(*call_site)
        ),
        Instruction::SendSuper {
            selector,
            argument_registers,
            destination,
            call_site,
        } => format!(
            "SendSuper r{destination} <- #{selector} ({}) {}",
            registers(argument_registers),
            format_call_site(*call_site)
        ),
        Instruction::MakeClosure {
            destination,
            parameters,
            ..
        } => format!(
            "MakeClosure r{destination}, [{}] ->",
            parameters
                .iter()
                .map(|parameter| format!(":{parameter}"))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Instruction::MakeArray {
            destination,
            element_registers,
        } => format!(
            "MakeArray r{destination}, ({})",
            registers(element_registers)
        ),
        Instruction::InitializeSlot { source, own_index } => {
            format!("InitSlot own {own_index} <- r{source}")
        }
        Instruction::Return { register } => format!("Return r{register}"),
        Instruction::ImplicitReturn { register } => format!("ImplicitReturn r{register}"),
        Instruction::NonLocalReturn { register } => format!("NonLocalReturn r{register}"),
        Instruction::Jump { target } => format!("Jump -> {target}"),
        Instruction::JumpIfNotBoolean { register, target } => {
            format!("JumpIfNotBoolean r{register} -> {target}")
        }
        Instruction::BranchIfFalse { register, target } => {
            format!("BranchIfFalse r{register} -> {target}")
        }
        Instruction::BranchIfTrue { register, target } => {
            format!("BranchIfTrue r{register} -> {target}")
        }
    }
}

/// A send's call site as `disasm` prints it: its family number, or `@-` for
/// `UNCACHED` -- the reference lowering's bodies carry no numbering at all.
fn format_call_site(call_site: CallSite) -> String {
    if call_site == CallSite::UNCACHED {
        "@-".to_string()
    } else {
        format!("@{}", call_site.number)
    }
}

fn format_literal(literal: &Literal) -> String {
    match literal {
        Literal::Nil => "nil".to_string(),
        Literal::Boolean(boolean) => boolean.to_string(),
        Literal::SmallInteger(integer) => integer.to_string(),
        Literal::Float(float) => format!("{float:?}"),
        Literal::String(string) => format!("'{string}'"),
        Literal::Symbol(symbol) => format!("#{symbol}"),
        Literal::Character(character) => format!("\"{character}\""),
        Literal::Array(elements) => format!(
            "#({})",
            elements
                .iter()
                .map(format_literal)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Literal::MalformedInteger(text) => format!("malformed-int '{text}'"),
        Literal::UnresolvedIdentifier(name) => format!("unresolved-ident '{name}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_code_construction_smoke_test() {
        let code = CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![
                Instruction::LoadLiteral {
                    destination: 1,
                    literal: Literal::SmallInteger(42),
                },
                Instruction::Return { register: 1 },
            ],
            argument_count: 1,
            register_count: 2,
            statement_program_counter: vec![(0, 0..2)],
            family_size: 0,
            source: Span::new(0, 10),
        };
        assert_eq!(code.instructions.len(), 2);
        assert_eq!(code.register_count - code.argument_count, 1);
        assert_eq!(code, code.clone());
    }

    #[test]
    fn call_site_number_is_copy_and_comparable() {
        let original = CallSite { number: 3 };
        let copy = original;
        assert_eq!(original, copy);
    }

    #[test]
    fn send_instruction_variants_carry_a_call_site() {
        let send = Instruction::SendPublic {
            selector: Symbol::intern("foo:"),
            receiver_register: 0,
            argument_registers: vec![1],
            destination: 2,
            call_site: CallSite { number: 0 },
        };
        assert!(matches!(
            send,
            Instruction::SendPublic {
                call_site: CallSite { number: 0 },
                ..
            }
        ));
    }

    #[test]
    fn make_closure_carries_compiled_block_code() {
        let block = Rc::new(CompiledCode {
            kind: CodeKind::Block,
            instructions: vec![Instruction::ImplicitReturn { register: 0 }],
            argument_count: 1,
            register_count: 1,
            statement_program_counter: vec![(0, 0..1)],
            family_size: 0,
            source: Span::new(0, 5),
        });
        let make = Instruction::MakeClosure {
            destination: 0,
            parameters: Rc::new(vec![Symbol::intern("x")]),
            code: block,
        };
        let Instruction::MakeClosure { code, .. } = &make else {
            panic!()
        };
        assert_eq!(code.kind, CodeKind::Block);
    }

    #[test]
    fn disassemble_lists_instructions_with_statement_labels_and_nested_blocks() {
        let block = Rc::new(CompiledCode {
            kind: CodeKind::Block,
            instructions: vec![
                Instruction::LoadOuterLocal {
                    destination: 0,
                    depth: 1,
                    index: 0,
                },
                Instruction::NonLocalReturn { register: 0 },
            ],
            argument_count: 0,
            register_count: 1,
            statement_program_counter: vec![(0, 0..2)],
            family_size: 0,
            source: Span::new(0, 5),
        });
        let code = CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![
                Instruction::MakeClosure {
                    destination: 1,
                    parameters: Rc::new(vec![]),
                    code: block,
                },
                Instruction::SendPublic {
                    selector: Symbol::intern("value"),
                    receiver_register: 1,
                    argument_registers: vec![],
                    destination: 2,
                    call_site: CallSite { number: 1 },
                },
                Instruction::LoadSelf { destination: 3 },
                Instruction::ImplicitReturn { register: 3 },
            ],
            argument_count: 1,
            register_count: 4,
            statement_program_counter: vec![(0, 0..2)],
            family_size: 2,
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
        let literals = vec![
            Literal::Nil,
            Literal::Boolean(true),
            Literal::SmallInteger(-3),
            Literal::Float(2.5),
            Literal::String("s".to_string()),
            Literal::Symbol(Symbol::intern("sym")),
            Literal::Character('c'),
            Literal::Array(vec![Literal::SmallInteger(1), Literal::Nil]),
            Literal::MalformedInteger("16r1.8".to_string()),
        ];
        let rendered: Vec<String> = literals.iter().map(format_literal).collect();
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
