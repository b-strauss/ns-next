//! Unwind markers (`docs/design/core-library.md`, "Planned: exceptions"):
//! `Block>>on:do:`, `ensure:` and `ifCurtailed:` are ordinary kernel
//! methods whose compiled code says what they are and where their
//! parameters and locals live, so that the unwinder -- and, later,
//! `signal` -- can find them on the stack and read them there. Recognized
//! the way the quick methods are, when the kernel is compiled, and only
//! there: the caller asks for it only for the kernel's `Block`
//! (`instantiate::build_methods`), so no program can mark a method of its
//! own.

use ns_syntax::ast;

use crate::bytecode::{CompiledCode, Instruction, UnwindMarker};

/// The marker `declaration`, a method of the kernel's `Block` compiled to
/// `code`, carries, if it is one of the three. The registers are read off
/// the layout `docs/design/compiler.md`, "Registers" gives a method: its
/// parameters from 0, then its named temporaries in declaration order.
pub(crate) fn recognize(
    declaration: &ast::MethodDeclaration,
    code: &CompiledCode,
) -> Option<UnwindMarker> {
    let selector = crate::interpreter::instantiate::method_name(&declaration.pattern);
    let marker = match selector.as_str() {
        "on:do:" => UnwindMarker::Handler {
            class_register: 0,
            block_register: 1,
            active_register: local_register(declaration, code, "handlerActive"),
        },
        "ensure:" => UnwindMarker::Ensure {
            block_register: 0,
            complete_register: local_register(declaration, code, "complete"),
        },
        "ifCurtailed:" => UnwindMarker::IfCurtailed {
            block_register: 0,
            complete_register: local_register(declaration, code, "complete"),
        },
        _ => return None,
    };
    // A local the capture analysis moves into an indirection vector keeps
    // its register but leaves it nil for good (`docs/design/compiler.md`,
    // "Registers"), so the unwinder would read `complete` as never set and
    // run the block a second time. Nothing in the three bodies captures a
    // local today; a change to `Kernel.ns` that did is refused here rather
    // than left to run an `ensure:` block twice.
    assert!(
        !code
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::MakeVector { .. })),
        "internal: Kernel.ns's Block>>{selector} keeps a local in an indirection vector, \
         where the unwinder cannot read it"
    );
    // The local's register is read off the layout, not off the code; the
    // body itself must write it there -- `ensure:`'s `complete:: true`,
    // `on:do:`'s `handlerActive ::= true` -- or a lowering change has moved
    // the local and the unwinder or `signal` would read another register.
    // `ifCurtailed:` writes nothing: its `complete` starts nil, which is
    // every register's initial value, and only the unwinder sets it.
    let written = match marker {
        UnwindMarker::Handler {
            active_register, ..
        } => Some(active_register),
        UnwindMarker::Ensure {
            complete_register, ..
        } => Some(complete_register),
        UnwindMarker::IfCurtailed { .. } => None,
    };
    if let Some(register) = written {
        debug_assert!(
            code.instructions
                .iter()
                .any(|instruction| writes_register(instruction, register)),
            "internal: Kernel.ns's Block>>{selector} never writes register {register}, \
             where its marker says the local lives"
        );
    }
    Some(marker)
}

/// Whether `instruction` stores into `register` of its own activation.
fn writes_register(instruction: &Instruction, register: u16) -> bool {
    match instruction {
        Instruction::StoreLocal { slot, .. } => *slot == register,
        Instruction::LoadLiteral { destination, .. } | Instruction::Move { destination, .. } => {
            *destination == register
        }
        _ => false,
    }
}

/// The register of the named temporary `name`, which the kernel method
/// must declare: a missing one is a defect in `Kernel.ns`.
fn local_register(declaration: &ast::MethodDeclaration, code: &CompiledCode, name: &str) -> u16 {
    let position = declaration
        .body
        .temporaries
        .iter()
        .position(|temporary| temporary.name.name == name)
        .unwrap_or_else(|| {
            panic!("internal: Kernel.ns's marked Block method declares no local {name}")
        });
    debug_assert_eq!(code.copied_count, 0, "a method is handed no copied values");
    code.argument_count + position as u16
}
