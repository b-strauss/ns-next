//! The machine-readable tables of the specification's bytecode chapter
//! (`docs/spec/bytecode.md`), which the tests of the instructions, their
//! wire encoding and their eight-byte form hold the code to, and of its
//! image-format chapter (`docs/spec/image-format.md`), which the image
//! tests hold the encoder to.

use crate::bytecode::Instruction;

mod image_format;

/// How many kinds of instruction there are: the arms of
/// `instruction_name`. A new instruction adds an arm there, which the
/// compiler demands, and raises this count beside it, which the tables'
/// tests then hold their samples -- and so the book's rows -- to.
pub(crate) const INSTRUCTION_KINDS: usize = 37;

/// The name of `instruction`'s kind, as the book's pages and rows give it.
/// The match has no `_` arm, so that a new instruction must be named here.
pub(crate) fn instruction_name(instruction: &Instruction) -> &'static str {
    match instruction {
        Instruction::LoadLiteral { .. } => "LoadLiteral",
        Instruction::LoadSelf { .. } => "LoadSelf",
        Instruction::LoadEnclosingObject { .. } => "LoadEnclosingObject",
        Instruction::Move { .. } => "Move",
        Instruction::StoreLocal { .. } => "StoreLocal",
        Instruction::LoadOuterLocal { .. } => "LoadOuterLocal",
        Instruction::StoreOuterLocal { .. } => "StoreOuterLocal",
        Instruction::SendLexical { .. } => "SendLexical",
        Instruction::SendPublic { .. } => "SendPublic",
        Instruction::SendInlinable { .. } => "SendInlinable",
        Instruction::SendSuper { .. } => "SendSuper",
        Instruction::SendMirror { .. } => "SendMirror",
        Instruction::MakeClosure { .. } => "MakeClosure",
        Instruction::MakeArray { .. } => "MakeArray",
        Instruction::MakeVector { .. } => "MakeVector",
        Instruction::LoadVector { .. } => "LoadVector",
        Instruction::StoreVector { .. } => "StoreVector",
        Instruction::InitializeSlot { .. } => "InitializeSlot",
        Instruction::ReturnSlotIfFilled { .. } => "ReturnSlotIfFilled",
        Instruction::MakeFuture { .. } => "MakeFuture",
        Instruction::ResolveFuture { .. } => "ResolveFuture",
        Instruction::LoadOwnSlot { .. } => "LoadOwnSlot",
        Instruction::InitializeSlotProvisionally { .. } => "InitializeSlotProvisionally",
        Instruction::Return { .. } => "Return",
        Instruction::ImplicitReturn { .. } => "ImplicitReturn",
        Instruction::NonLocalReturn { .. } => "NonLocalReturn",
        Instruction::SignalNonBoolean { .. } => "SignalNonBoolean",
        Instruction::Jump { .. } => "Jump",
        Instruction::JumpIfNotBoolean { .. } => "JumpIfNotBoolean",
        Instruction::BranchIfFalse { .. } => "BranchIfFalse",
        Instruction::BranchIfTrue { .. } => "BranchIfTrue",
        Instruction::IntegerBinary { .. } => "IntegerBinary",
        Instruction::AllocateInstance { .. } => "AllocateInstance",
        Instruction::InitializeInstance { .. } => "InitializeInstance",
        Instruction::CheckAppliedClause { .. } => "CheckAppliedClause",
        Instruction::InitializeSuperclass { .. } => "InitializeSuperclass",
        Instruction::InitializeAppliedLevel { .. } => "InitializeAppliedLevel",
    }
}

/// The bytecode chapter, as the book has it.
const BYTECODE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/spec/bytecode.md"
));

/// The image-format chapter, as the book has it.
const IMAGE_FORMAT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/spec/image-format.md"
));

/// The rows of the bytecode chapter's table `name`; panics when the
/// chapter has no such table.
pub(crate) fn bytecode_table(name: &str) -> Vec<Vec<String>> {
    table(BYTECODE, "bytecode", name)
}

/// The rows of the image-format chapter's table `name`; panics when the
/// chapter has no such table.
pub(crate) fn image_table(name: &str) -> Vec<Vec<String>> {
    table(IMAGE_FORMAT, "image", name)
}

/// The rows of `chapter`'s fenced block whose info string is
/// `spec-table <name>`, each non-empty line split at whitespace; panics,
/// naming the chapter by `chapter_name`, when it has no such block.
fn table(chapter: &str, chapter_name: &str, name: &str) -> Vec<Vec<String>> {
    let opening = format!("```spec-table {name}");
    let mut lines = chapter
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .skip_while(|line| line.trim() != opening);
    assert!(
        lines.next().is_some(),
        "the {chapter_name} chapter has no table `{name}`"
    );
    lines
        .take_while(|line| line.trim() != "```")
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split_whitespace().map(str::to_string).collect())
        .collect()
}
