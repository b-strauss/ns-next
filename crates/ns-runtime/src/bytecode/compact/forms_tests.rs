//! The specification's eight-byte forms (`docs/spec/bytecode.md`, "The
//! eight-byte form", `bytecode.compact.forms`) held to `CompactInstruction`:
//! every variant, its fields in order and each field's width.

use super::*;
use crate::spec_tables::bytecode_table;

/// The name of `form`'s variant. The match has no `_` arm, so that a new
/// variant fails to compile until it is named here -- and then, unless
/// `every_form` builds one and the book's table has its row, the test.
fn form_name(form: CompactInstruction) -> &'static str {
    match form {
        CompactInstruction::LoadNil { .. } => "LoadNil",
        CompactInstruction::LoadTrue { .. } => "LoadTrue",
        CompactInstruction::LoadFalse { .. } => "LoadFalse",
        CompactInstruction::LoadSmallInteger { .. } => "LoadSmallInteger",
        CompactInstruction::LoadConstant { .. } => "LoadConstant",
        CompactInstruction::LoadLiteral { .. } => "LoadLiteral",
        CompactInstruction::LoadSelf { .. } => "LoadSelf",
        CompactInstruction::LoadEnclosingObject { .. } => "LoadEnclosingObject",
        CompactInstruction::Move { .. } => "Move",
        CompactInstruction::StoreLocal { .. } => "StoreLocal",
        CompactInstruction::LoadOuterLocal { .. } => "LoadOuterLocal",
        CompactInstruction::StoreOuterLocal { .. } => "StoreOuterLocal",
        CompactInstruction::SendPublic0 { .. } => "SendPublic0",
        CompactInstruction::SendPublic1 { .. } => "SendPublic1",
        CompactInstruction::SendPublic2 { .. } => "SendPublic2",
        CompactInstruction::SendPublicWide { .. } => "SendPublicWide",
        CompactInstruction::SendInlinable0 { .. } => "SendInlinable0",
        CompactInstruction::SendInlinable1 { .. } => "SendInlinable1",
        CompactInstruction::SendInlinable2 { .. } => "SendInlinable2",
        CompactInstruction::SendInlinableWide { .. } => "SendInlinableWide",
        CompactInstruction::SendLexical0 { .. } => "SendLexical0",
        CompactInstruction::SendLexical1 { .. } => "SendLexical1",
        CompactInstruction::SendLexical2 { .. } => "SendLexical2",
        CompactInstruction::SendLexicalWide { .. } => "SendLexicalWide",
        CompactInstruction::SendSuper0 { .. } => "SendSuper0",
        CompactInstruction::SendSuper1 { .. } => "SendSuper1",
        CompactInstruction::SendSuper2 { .. } => "SendSuper2",
        CompactInstruction::SendSuperWide { .. } => "SendSuperWide",
        CompactInstruction::SendMirror { .. } => "SendMirror",
        CompactInstruction::MakeClosure { .. } => "MakeClosure",
        CompactInstruction::MakeArray { .. } => "MakeArray",
        CompactInstruction::MakeVector { .. } => "MakeVector",
        CompactInstruction::LoadVector { .. } => "LoadVector",
        CompactInstruction::StoreVector { .. } => "StoreVector",
        CompactInstruction::InitializeSlot { .. } => "InitializeSlot",
        CompactInstruction::ReturnSlotIfFilled { .. } => "ReturnSlotIfFilled",
        CompactInstruction::MakeFuture { .. } => "MakeFuture",
        CompactInstruction::ResolveFuture { .. } => "ResolveFuture",
        CompactInstruction::LoadOwnSlot { .. } => "LoadOwnSlot",
        CompactInstruction::InitializeSlotProvisionally { .. } => "InitializeSlotProvisionally",
        CompactInstruction::Return { .. } => "Return",
        CompactInstruction::ImplicitReturn { .. } => "ImplicitReturn",
        CompactInstruction::NonLocalReturn { .. } => "NonLocalReturn",
        CompactInstruction::SignalNonBoolean { .. } => "SignalNonBoolean",
        CompactInstruction::Jump { .. } => "Jump",
        CompactInstruction::JumpIfNotBoolean { .. } => "JumpIfNotBoolean",
        CompactInstruction::BranchIfFalse { .. } => "BranchIfFalse",
        CompactInstruction::BranchIfTrue { .. } => "BranchIfTrue",
        CompactInstruction::IntegerAdd { .. } => "IntegerAdd",
        CompactInstruction::IntegerSubtract { .. } => "IntegerSubtract",
        CompactInstruction::IntegerMultiply { .. } => "IntegerMultiply",
        CompactInstruction::IntegerFloorDivide { .. } => "IntegerFloorDivide",
        CompactInstruction::IntegerLess { .. } => "IntegerLess",
        CompactInstruction::IntegerGreater { .. } => "IntegerGreater",
        CompactInstruction::IntegerLessOrEqual { .. } => "IntegerLessOrEqual",
        CompactInstruction::IntegerGreaterOrEqual { .. } => "IntegerGreaterOrEqual",
        CompactInstruction::IntegerEqual { .. } => "IntegerEqual",
        CompactInstruction::AllocateInstance { .. } => "AllocateInstance",
        CompactInstruction::InitializeInstance { .. } => "InitializeInstance",
        CompactInstruction::CheckAppliedClause { .. } => "CheckAppliedClause",
        CompactInstruction::InitializeSuperclass { .. } => "InitializeSuperclass",
        CompactInstruction::InitializeAppliedLevel { .. } => "InitializeAppliedLevel",
        CompactInstruction::Wide { .. } => "Wide",
        CompactInstruction::Breakpoint => "Breakpoint",
    }
}

const BYTE: u8 = u8::MAX;
const HALF: u16 = u16::MAX;
const WORD: u32 = u32::MAX;

/// One value of every variant, each integer field at its type's maximum
/// (an `i32` at its minimum), so that the width the book gives a field is
/// the one whose extreme it prints. A new variant goes here and into
/// `form_name`.
// One value per variant, read top to bottom.
#[expect(
    clippy::too_many_lines,
    reason = "one value per variant, each a line or a few"
)]
fn every_form() -> Vec<CompactInstruction> {
    use CompactInstruction as Form;
    let selector = Symbol::intern("with:with:");
    vec![
        Form::LoadNil { destination: BYTE },
        Form::LoadTrue { destination: BYTE },
        Form::LoadFalse { destination: BYTE },
        Form::LoadSmallInteger {
            destination: BYTE,
            value: i32::MIN,
        },
        Form::LoadConstant {
            destination: BYTE,
            constant: HALF,
        },
        Form::LoadLiteral {
            destination: BYTE,
            literal: HALF,
        },
        Form::LoadSelf { destination: BYTE },
        Form::LoadEnclosingObject {
            destination: BYTE,
            depth: HALF,
        },
        Form::Move {
            destination: BYTE,
            source: BYTE,
        },
        Form::StoreLocal {
            source: BYTE,
            slot: BYTE,
        },
        Form::LoadOuterLocal {
            destination: BYTE,
            index: BYTE,
            depth: HALF,
        },
        Form::StoreOuterLocal {
            source: BYTE,
            index: BYTE,
            depth: HALF,
        },
        Form::SendPublic0 {
            destination: BYTE,
            receiver: BYTE,
            argument_base: BYTE,
            site: HALF,
        },
        Form::SendPublic1 {
            destination: BYTE,
            receiver: BYTE,
            argument_base: BYTE,
            source: BYTE,
            site: HALF,
        },
        Form::SendPublic2 {
            destination: BYTE,
            receiver: BYTE,
            argument_base: BYTE,
            sources: [BYTE; 2],
            site: HALF,
        },
        Form::SendPublicWide { site: WORD },
        Form::SendInlinable0 {
            destination: BYTE,
            receiver: BYTE,
            argument_base: BYTE,
            site: HALF,
        },
        Form::SendInlinable1 {
            destination: BYTE,
            receiver: BYTE,
            argument_base: BYTE,
            source: BYTE,
            site: HALF,
        },
        Form::SendInlinable2 {
            destination: BYTE,
            receiver: BYTE,
            argument_base: BYTE,
            sources: [BYTE; 2],
            site: HALF,
        },
        Form::SendInlinableWide { site: WORD },
        Form::SendLexical0 {
            destination: BYTE,
            argument_base: BYTE,
            site: HALF,
        },
        Form::SendLexical1 {
            destination: BYTE,
            argument_base: BYTE,
            source: BYTE,
            site: HALF,
        },
        Form::SendLexical2 {
            destination: BYTE,
            argument_base: BYTE,
            sources: [BYTE; 2],
            site: HALF,
        },
        Form::SendLexicalWide { site: WORD },
        Form::SendSuper0 {
            destination: BYTE,
            argument_base: BYTE,
            site: HALF,
        },
        Form::SendSuper1 {
            destination: BYTE,
            argument_base: BYTE,
            source: BYTE,
            site: HALF,
        },
        Form::SendSuper2 {
            destination: BYTE,
            argument_base: BYTE,
            sources: [BYTE; 2],
            site: HALF,
        },
        Form::SendSuperWide { site: WORD },
        Form::SendMirror {
            destination: BYTE,
            mirror: BYTE,
            subject: BYTE,
            primitive: MirrorPrimitive::from_index(0),
            site: HALF,
        },
        Form::MakeClosure {
            destination: BYTE,
            block: HALF,
        },
        Form::MakeArray {
            destination: BYTE,
            elements: HALF,
        },
        Form::MakeVector {
            destination: BYTE,
            size: HALF,
        },
        Form::LoadVector {
            destination: BYTE,
            vector: BYTE,
            index: HALF,
        },
        Form::StoreVector {
            vector: BYTE,
            source: BYTE,
            index: HALF,
        },
        Form::InitializeSlot {
            source: BYTE,
            own_index: WORD,
        },
        Form::ReturnSlotIfFilled { own_index: WORD },
        Form::MakeFuture {
            destination: BYTE,
            closure: BYTE,
        },
        Form::ResolveFuture {
            destination: BYTE,
            source: BYTE,
        },
        Form::LoadOwnSlot {
            destination: BYTE,
            own_index: WORD,
        },
        Form::InitializeSlotProvisionally {
            source: BYTE,
            own_index: WORD,
        },
        Form::Return { register: BYTE },
        Form::ImplicitReturn { register: BYTE },
        Form::NonLocalReturn { register: BYTE },
        Form::SignalNonBoolean { register: BYTE },
        Form::Jump { target: WORD },
        Form::JumpIfNotBoolean {
            register: BYTE,
            target: WORD,
        },
        Form::BranchIfFalse {
            register: BYTE,
            target: WORD,
        },
        Form::BranchIfTrue {
            register: BYTE,
            target: WORD,
        },
        Form::IntegerAdd {
            destination: BYTE,
            left: BYTE,
            right: BYTE,
            target: WORD,
        },
        Form::IntegerSubtract {
            destination: BYTE,
            left: BYTE,
            right: BYTE,
            target: WORD,
        },
        Form::IntegerMultiply {
            destination: BYTE,
            left: BYTE,
            right: BYTE,
            target: WORD,
        },
        Form::IntegerFloorDivide {
            destination: BYTE,
            left: BYTE,
            right: BYTE,
            target: WORD,
        },
        Form::IntegerLess {
            destination: BYTE,
            left: BYTE,
            right: BYTE,
            target: WORD,
        },
        Form::IntegerGreater {
            destination: BYTE,
            left: BYTE,
            right: BYTE,
            target: WORD,
        },
        Form::IntegerLessOrEqual {
            destination: BYTE,
            left: BYTE,
            right: BYTE,
            target: WORD,
        },
        Form::IntegerGreaterOrEqual {
            destination: BYTE,
            left: BYTE,
            right: BYTE,
            target: WORD,
        },
        Form::IntegerEqual {
            destination: BYTE,
            left: BYTE,
            right: BYTE,
            target: WORD,
        },
        Form::AllocateInstance { destination: BYTE },
        Form::InitializeInstance {
            destination: BYTE,
            instance: BYTE,
        },
        Form::CheckAppliedClause {
            steps: HALF,
            selector,
        },
        Form::InitializeSuperclass {
            destination: BYTE,
            site: HALF,
        },
        Form::InitializeAppliedLevel {
            destination: BYTE,
            argument_base: BYTE,
            steps: HALF,
            arguments: HALF,
        },
        Form::Wide { index: WORD },
        Form::Breakpoint,
    ]
}

/// A form's fields as its `Debug` prints them: name and value, in
/// declaration order, a field holding an array kept whole.
fn printed_fields(form: CompactInstruction) -> Vec<(String, String)> {
    let printed = format!("{form:?}");
    let Some((_, body)) = printed.split_once(" { ") else {
        return Vec::new();
    };
    let body = body.strip_suffix(" }").expect("a struct variant's braces");
    let mut fields = Vec::new();
    let mut depth = 0_i32;
    let mut current = String::new();
    for character in body.chars() {
        match character {
            '[' => depth += 1,
            ']' => depth -= 1,
            _ => {}
        }
        if character == ',' && depth == 0 {
            fields.push(std::mem::take(&mut current));
        } else {
            current.push(character);
        }
    }
    fields.push(current);
    fields
        .iter()
        .map(|field| {
            let (name, value) = field.trim().split_once(": ").expect("`name: value`");
            (name.to_string(), value.to_string())
        })
        .collect()
}

/// What a field of the book's `type` prints as at the extreme `every_form`
/// gives it; `None` for a type whose value is not an integer.
fn extreme_of(kind: &str) -> Option<String> {
    match kind {
        "u8" => Some(u8::MAX.to_string()),
        "u16" => Some(u16::MAX.to_string()),
        "u32" => Some(u32::MAX.to_string()),
        "i32" => Some(i32::MIN.to_string()),
        "u8x2" => Some(format!("[{0}, {0}]", u8::MAX)),
        "symbol" | "primitive" => None,
        other => panic!("the eight-byte forms have no type `{other}`"),
    }
}

// spec: bytecode.compact.forms
#[test]
fn every_eight_byte_form_is_the_books() {
    let rows = bytecode_table("compact-forms");
    let forms = every_form();
    let mut names: Vec<&str> = forms.iter().copied().map(form_name).collect();
    let count = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), count, "every form is built once");
    let mut book: Vec<&str> = rows.iter().map(|row| row[0].as_str()).collect();
    book.sort_unstable();
    assert_eq!(book, names, "one row per form");
    for &form in &forms {
        let name = form_name(form);
        let row = rows
            .iter()
            .find(|row| row[0] == name)
            .expect("a row per form");
        let printed = printed_fields(form);
        assert_eq!(
            row.len() - 1,
            printed.len(),
            "{name}: the book's fields are the form's"
        );
        for (cell, (field, value)) in row[1..].iter().zip(&printed) {
            let (book_field, kind) = cell
                .split_once(':')
                .unwrap_or_else(|| panic!("{name}: `{cell}` is no `field:type`"));
            assert_eq!(book_field, field, "{name}: the fields' order");
            if let Some(extreme) = extreme_of(kind) {
                assert_eq!(&extreme, value, "{name}: the width of `{field}`");
            }
        }
    }
}
