//! The shipped modules' compiled mixins as the artifact's bytes (the format
//! is `shipped_code`'s doc comment). Every type is destructured without `..`
//! and every enum matched without `_`, so a new field or variant does not
//! compile until its treatment in the artifact is decided.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast::AccessModifier;
use ns_syntax::Span;

use crate::bytecode::{
    CallSite, CodeKind, CompiledCode, CompiledCodeFields, Instruction, IntegerOperation, Literal,
    UnwindMarker,
};
use crate::mixin::{
    AccessorKind, ClassAccessor, ClauseMessage, Factory, FactoryMethod, MethodEntry,
    MethodFastPath, Mixin, MixinApplicationClause, MixinMethod, MixinSlot, QuickConstant,
    SlotAccessor,
};
use crate::resolver::RootShape;
use crate::types::BuiltinClass;
use crate::Symbol;

use super::bytes::ByteWriter;
use super::{FORMAT_VERSION, MAGIC};

/// The artifact of `modules`, in their order, under `fingerprint`.
pub(crate) fn encode(modules: &[(&'static str, Rc<Mixin>)], fingerprint: u64) -> Vec<u8> {
    let mut writer = ByteWriter::new();
    writer.raw(&MAGIC);
    writer.u16(FORMAT_VERSION);
    writer.u64(fingerprint);
    writer.length(modules.len());
    for (name, mixin) in modules {
        let section = encode_section(mixin);
        writer.str(name);
        writer.length(section.len());
        writer.raw(&section);
    }
    writer.into_bytes()
}

/// One module's section: its symbol table, then its root mixin. The mixin
/// is written first, into a writer of its own, because only writing it
/// collects the symbols the table that precedes it holds.
fn encode_section(mixin: &Mixin) -> Vec<u8> {
    let mut out = Out {
        writer: ByteWriter::new(),
        symbols: SymbolTable {
            indices: HashMap::new(),
            names: Vec::new(),
        },
    };
    write_mixin(&mut out, mixin);
    let Out { writer, symbols } = out;
    let mut section = ByteWriter::new();
    section.length(symbols.names.len());
    for name in &symbols.names {
        section.str(name);
    }
    section.raw(&writer.into_bytes());
    section.into_bytes()
}

/// A section's symbols, in order of first use; a symbol is written as its
/// index here.
struct SymbolTable {
    indices: HashMap<&'static str, u32>,
    names: Vec<&'static str>,
}

impl SymbolTable {
    fn index(&mut self, symbol: Symbol) -> u32 {
        let name = symbol.as_str();
        if let Some(index) = self.indices.get(name) {
            return *index;
        }
        let index = u32::try_from(self.names.len()).expect("internal: symbol count fits a u32");
        self.indices.insert(name, index);
        self.names.push(name);
        index
    }
}

/// A section's mixin bytes being written, and the symbols they name.
struct Out {
    writer: ByteWriter,
    symbols: SymbolTable,
}

fn write_mixin(out: &mut Out, mixin: &Mixin) {
    let Mixin {
        name,
        access,
        methods,
        slots,
        factory,
        superclass,
        superclass_receiver,
        mixin_applications,
        has_initializer_expressions,
        // Read only by the compiler, which has run.
        initializer_expressions: _,
        // A tree no heap registered: read through `declared_nested`.
        nested_declarations: _,
        class_side,
        span,
        // Read only by the compiler and the source path's checks.
        declaration: _,
        initializer_code,
        own_initializer_code,
        standalone_initializer_code,
        superclass_clause_code,
        superclass_message_code,
        root_clause_defect,
        root_shape,
        shipped,
        // Set by the decoder: the tree gives the link, and the binary's
        // own text, with the spans, each declaration's.
        enclosing: _,
        text: _,
    } = mixin;
    assert!(
        root_clause_defect.is_none(),
        "internal: a shipped root passed the resolver, which refuses a defect"
    );
    out.symbol(*name);
    out.access(*access);
    // Sorted by selector name: a `HashMap`'s order is not the bytes'.
    let mut entries: Vec<(&Symbol, &MethodEntry)> = methods.iter().collect();
    entries.sort_by_key(|(selector, _)| selector.as_str());
    out.length(entries.len());
    for (selector, entry) in entries {
        out.symbol(*selector);
        out.method_entry(entry);
    }
    out.length(slots.len());
    for slot in slots {
        let MixinSlot {
            name,
            mutable,
            access,
            lazy,
            // Read only by the compiler, which has run.
            declaration: _,
        } = slot;
        out.symbol(*name);
        out.boolean(*mutable);
        out.access(*access);
        out.boolean(*lazy);
    }
    out.option(factory.as_ref(), |out, factory| {
        let Factory {
            selector,
            argument_count,
        } = factory;
        out.symbol(*selector);
        out.length(*argument_count);
    });
    out.option(superclass.as_ref(), |out, message| {
        out.clause_message(*message)
    });
    out.option(superclass_receiver.as_ref(), |out, receiver| {
        out.symbol(*receiver);
    });
    out.length(mixin_applications.len());
    for clause in mixin_applications {
        let MixinApplicationClause {
            message,
            receiver,
            message_code,
        } = clause;
        out.clause_message(*message);
        out.option(receiver.as_ref(), |out, receiver| out.symbol(*receiver));
        out.option(message_code.as_ref(), |out, code| out.code(code));
    }
    out.boolean(*has_initializer_expressions);
    let nested = mixin.declared_nested();
    out.length(nested.len());
    for nested in nested.iter() {
        write_mixin(out, nested);
    }
    out.option(class_side.as_ref(), |out, class_side| {
        write_mixin(out, class_side);
    });
    out.span(*span);
    out.option(initializer_code.as_ref(), |out, code| out.code(code));
    out.option(own_initializer_code.as_ref(), |out, code| out.code(code));
    out.option(standalone_initializer_code.as_ref(), |out, code| {
        out.code(code);
    });
    out.option(superclass_clause_code.as_ref(), |out, code| out.code(code));
    out.option(superclass_message_code.as_ref(), |out, code| out.code(code));
    out.option(root_shape.as_ref(), |out, shape| {
        let RootShape {
            over_value,
            applies_value,
        } = shape;
        out.boolean(*over_value);
        out.boolean(*applies_value);
    });
    out.boolean(*shipped);
}

impl Out {
    fn u8(&mut self, value: u8) {
        self.writer.u8(value);
    }

    fn boolean(&mut self, value: bool) {
        self.writer.boolean(value);
    }

    fn u16(&mut self, value: u16) {
        self.writer.u16(value);
    }

    fn u32(&mut self, value: u32) {
        self.writer.u32(value);
    }

    fn u64(&mut self, value: u64) {
        self.writer.u64(value);
    }

    fn length(&mut self, value: usize) {
        self.writer.length(value);
    }

    fn str(&mut self, text: &str) {
        self.writer.str(text);
    }

    fn symbol(&mut self, symbol: Symbol) {
        let index = self.symbols.index(symbol);
        self.writer.u32(index);
    }

    /// A `0` byte for `None`, else a `1` byte and the value `write` writes.
    fn option<T>(&mut self, value: Option<T>, write: impl FnOnce(&mut Out, T)) {
        match value {
            None => self.u8(0),
            Some(value) => {
                self.u8(1);
                write(self, value);
            }
        }
    }

    fn access(&mut self, access: AccessModifier) {
        self.u8(match access {
            AccessModifier::Public => 0,
            AccessModifier::Protected => 1,
            AccessModifier::Private => 2,
        });
    }

    fn clause_message(&mut self, message: ClauseMessage) {
        let ClauseMessage {
            selector,
            has_arguments,
        } = message;
        self.symbol(selector);
        self.boolean(has_arguments);
    }

    fn span(&mut self, span: Span) {
        let Span { start, end } = span;
        self.u32(start);
        self.u32(end);
    }

    fn method_entry(&mut self, entry: &MethodEntry) {
        match entry {
            MethodEntry::Compiled(method) => {
                let MixinMethod {
                    selector,
                    access,
                    argument_count,
                    // Read only by the compiler.
                    declaration: _,
                    code,
                    fast_path,
                    class_accessor,
                } = &**method;
                self.u8(0);
                self.symbol(*selector);
                self.access(*access);
                self.length(*argument_count);
                self.code(code);
                self.option(fast_path.as_ref(), |out, fast_path| {
                    out.fast_path(*fast_path);
                });
                self.option(class_accessor.as_ref(), |out, accessor| {
                    let ClassAccessor { depth, class_name } = accessor;
                    out.u16(*depth);
                    out.symbol(*class_name);
                });
            }
            MethodEntry::SlotAccessor(SlotAccessor {
                selector,
                access,
                own_index,
                kind,
            }) => {
                self.u8(1);
                self.symbol(*selector);
                self.access(*access);
                self.u32(*own_index);
                self.u8(match kind {
                    AccessorKind::Getter => 0,
                    AccessorKind::Setter => 1,
                });
            }
            MethodEntry::Factory(FactoryMethod {
                selector,
                argument_count,
                // Always public, and the body is `compiler::factory_body`
                // of the arity: `mixin::factory_entry` gives both back.
                access: _,
                code: _,
            }) => {
                self.u8(2);
                self.symbol(*selector);
                self.length(*argument_count);
            }
            MethodEntry::Primitive(_) => {
                panic!("internal: only the mirror's class holds a primitive, and Rust builds it")
            }
        }
    }

    fn fast_path(&mut self, fast_path: MethodFastPath) {
        match fast_path {
            MethodFastPath::Mirror(primitive) => {
                self.u8(0);
                self.mirror_primitive(primitive);
            }
            MethodFastPath::BuiltinClass(kind) => {
                self.u8(1);
                let position = BuiltinClass::ALL
                    .iter()
                    .position(|candidate| *candidate == kind)
                    .expect("internal: every builtin is in BuiltinClass::ALL");
                self.u8(u8::try_from(position).expect("internal: a builtin's position fits a u8"));
            }
            MethodFastPath::Constant(constant) => {
                self.u8(2);
                self.u8(match constant {
                    QuickConstant::True => 0,
                    QuickConstant::False => 1,
                    QuickConstant::Nil => 2,
                });
            }
            MethodFastPath::Receiver => self.u8(3),
            MethodFastPath::Parameter(position) => {
                self.u8(4);
                self.u8(position);
            }
            MethodFastPath::ValueOfParameter(position) => {
                self.u8(5);
                self.u8(position);
            }
            MethodFastPath::CullParameter(position) => {
                self.u8(6);
                self.u8(position);
            }
        }
    }

    fn mirror_primitive(&mut self, primitive: crate::builtins::primitive::MirrorPrimitive) {
        self.u16(u16::try_from(primitive.index()).expect("internal: a mirror row fits a u16"));
    }

    fn code(&mut self, code: &CompiledCode) {
        let instructions = code.instructions();
        let CompiledCodeFields {
            kind,
            // Its decoded view, `instructions` above, is what is written, so
            // the in-memory form, wide or narrow, is the decoder's
            // `CodeBuilder`'s to choose.
            body: _,
            argument_count,
            register_count,
            statement_program_counter,
            // All three derived from the instructions by `from_builder`, as the
            // compiler derives them, so `outer_reach_of` and
            // `addresses_own_slots_of` decide them here too.
            family_size: _,
            copied_count,
            outer_reach: _,
            addresses_own_slots: _,
            unwind_marker,
            source,
            // A heap's answer, valid only in that heap; the decoded body
            // starts with none.
            code_id_cache: _,
        } = code.fields();
        self.u8(match kind {
            CodeKind::Method => 0,
            CodeKind::Block => 1,
            CodeKind::Initializer => 2,
            CodeKind::SuperclassClause => 3,
            CodeKind::SuperclassMessage => 4,
        });
        self.u16(*argument_count);
        self.u16(*register_count);
        self.u16(*copied_count);
        self.length(statement_program_counter.len());
        for (statement, range) in statement_program_counter {
            self.length(*statement);
            self.length(range.start);
            self.length(range.end);
        }
        self.option(unwind_marker.as_ref(), |out, marker| {
            out.unwind_marker(*marker);
        });
        self.span(*source);
        self.length(instructions.len());
        for instruction in &instructions {
            self.instruction(instruction);
        }
    }

    fn unwind_marker(&mut self, marker: UnwindMarker) {
        match marker {
            UnwindMarker::Handler {
                class_register,
                block_register,
                active_register,
            } => {
                self.u8(0);
                self.u16(class_register);
                self.u16(block_register);
                self.u16(active_register);
            }
            UnwindMarker::Ensure {
                block_register,
                complete_register,
            } => {
                self.u8(1);
                self.u16(block_register);
                self.u16(complete_register);
            }
            UnwindMarker::IfCurtailed {
                block_register,
                complete_register,
            } => {
                self.u8(2);
                self.u16(block_register);
                self.u16(complete_register);
            }
        }
    }

    fn registers(&mut self, registers: &[u16]) {
        self.length(registers.len());
        for register in registers {
            self.u16(*register);
        }
    }

    fn call_site(&mut self, call_site: CallSite) {
        let CallSite { number } = call_site;
        self.u32(number);
    }

    /// One tag per variant, in `Instruction`'s declaration order, then every
    /// field in declaration order.
    fn instruction(&mut self, instruction: &Instruction) {
        match instruction {
            Instruction::LoadLiteral {
                destination,
                literal,
            } => {
                self.u8(0);
                self.u16(*destination);
                self.literal(literal);
            }
            Instruction::LoadSelf { destination } => {
                self.u8(1);
                self.u16(*destination);
            }
            Instruction::LoadEnclosingObject { destination, depth } => {
                self.u8(2);
                self.u16(*destination);
                self.u16(*depth);
            }
            Instruction::Move {
                destination,
                source,
            } => {
                self.u8(3);
                self.u16(*destination);
                self.u16(*source);
            }
            Instruction::StoreLocal { source, slot } => {
                self.u8(4);
                self.u16(*source);
                self.u16(*slot);
            }
            Instruction::LoadOuterLocal {
                destination,
                depth,
                index,
            } => {
                self.u8(5);
                self.u16(*destination);
                self.u16(*depth);
                self.u16(*index);
            }
            Instruction::StoreOuterLocal {
                source,
                depth,
                index,
            } => {
                self.u8(6);
                self.u16(*source);
                self.u16(*depth);
                self.u16(*index);
            }
            Instruction::SendLexical {
                depth,
                selector,
                argument_registers,
                destination,
                call_site,
            } => {
                self.u8(7);
                self.u16(*depth);
                self.symbol(*selector);
                self.registers(argument_registers);
                self.u16(*destination);
                self.call_site(*call_site);
            }
            Instruction::SendPublic {
                selector,
                receiver_register,
                argument_registers,
                destination,
                call_site,
            } => {
                self.u8(8);
                self.symbol(*selector);
                self.u16(*receiver_register);
                self.registers(argument_registers);
                self.u16(*destination);
                self.call_site(*call_site);
            }
            Instruction::SendInlinable {
                selector,
                receiver_register,
                argument_registers,
                destination,
                call_site,
            } => {
                self.u8(9);
                self.symbol(*selector);
                self.u16(*receiver_register);
                self.registers(argument_registers);
                self.u16(*destination);
                self.call_site(*call_site);
            }
            Instruction::SendSuper {
                selector,
                argument_registers,
                destination,
                call_site,
            } => {
                self.u8(10);
                self.symbol(*selector);
                self.registers(argument_registers);
                self.u16(*destination);
                self.call_site(*call_site);
            }
            Instruction::SendMirror {
                primitive,
                mirror_register,
                subject_register,
                argument_registers,
                destination,
                failure,
            } => {
                self.u8(11);
                self.mirror_primitive(*primitive);
                self.u16(*mirror_register);
                self.u16(*subject_register);
                self.registers(argument_registers);
                self.u16(*destination);
                self.u32(*failure);
            }
            Instruction::MakeClosure {
                destination,
                parameters,
                code,
                copied,
            } => {
                self.u8(12);
                self.u16(*destination);
                self.length(parameters.len());
                for parameter in parameters.iter() {
                    self.symbol(*parameter);
                }
                self.code(code);
                self.registers(copied);
            }
            Instruction::MakeArray {
                destination,
                element_registers,
            } => {
                self.u8(13);
                self.u16(*destination);
                self.registers(element_registers);
            }
            Instruction::MakeVector { destination, size } => {
                self.u8(14);
                self.u16(*destination);
                self.u16(*size);
            }
            Instruction::LoadVector {
                destination,
                vector,
                index,
            } => {
                self.u8(15);
                self.u16(*destination);
                self.u16(*vector);
                self.u16(*index);
            }
            Instruction::StoreVector {
                vector,
                index,
                source,
            } => {
                self.u8(16);
                self.u16(*vector);
                self.u16(*index);
                self.u16(*source);
            }
            Instruction::InitializeSlot { source, own_index } => {
                self.u8(17);
                self.u16(*source);
                self.u32(*own_index);
            }
            Instruction::ReturnSlotIfFilled { own_index } => {
                self.u8(18);
                self.u32(*own_index);
            }
            Instruction::MakeFuture {
                destination,
                closure,
            } => {
                self.u8(19);
                self.u16(*destination);
                self.u16(*closure);
            }
            Instruction::ResolveFuture {
                destination,
                source,
            } => {
                self.u8(20);
                self.u16(*destination);
                self.u16(*source);
            }
            Instruction::LoadOwnSlot {
                destination,
                own_index,
            } => {
                self.u8(21);
                self.u16(*destination);
                self.u32(*own_index);
            }
            Instruction::InitializeSlotProvisionally { source, own_index } => {
                self.u8(22);
                self.u16(*source);
                self.u32(*own_index);
            }
            Instruction::Return { register } => {
                self.u8(23);
                self.u16(*register);
            }
            Instruction::ImplicitReturn { register } => {
                self.u8(24);
                self.u16(*register);
            }
            Instruction::NonLocalReturn { register } => {
                self.u8(25);
                self.u16(*register);
            }
            Instruction::SignalNonBoolean { register } => {
                self.u8(26);
                self.u16(*register);
            }
            Instruction::Jump { target } => {
                self.u8(27);
                self.u32(*target);
            }
            Instruction::JumpIfNotBoolean { register, target } => {
                self.u8(28);
                self.u16(*register);
                self.u32(*target);
            }
            Instruction::BranchIfFalse { register, target } => {
                self.u8(29);
                self.u16(*register);
                self.u32(*target);
            }
            Instruction::BranchIfTrue { register, target } => {
                self.u8(30);
                self.u16(*register);
                self.u32(*target);
            }
            Instruction::IntegerBinary {
                operation,
                destination,
                left,
                right,
                target,
            } => {
                self.u8(31);
                self.u8(match operation {
                    IntegerOperation::Add => 0,
                    IntegerOperation::Subtract => 1,
                    IntegerOperation::Multiply => 2,
                    IntegerOperation::FloorDivide => 3,
                    IntegerOperation::Less => 4,
                    IntegerOperation::Greater => 5,
                    IntegerOperation::LessOrEqual => 6,
                    IntegerOperation::GreaterOrEqual => 7,
                    IntegerOperation::Equal => 8,
                });
                self.u16(*destination);
                self.u16(*left);
                self.u16(*right);
                self.u32(*target);
            }
            Instruction::AllocateInstance { destination } => {
                self.u8(32);
                self.u16(*destination);
            }
            Instruction::InitializeInstance {
                instance,
                argument_registers,
                destination,
            } => {
                self.u8(33);
                self.u16(*instance);
                self.registers(argument_registers);
                self.u16(*destination);
            }
            Instruction::CheckAppliedClause { steps, selector } => {
                self.u8(34);
                self.u16(*steps);
                self.symbol(*selector);
            }
            Instruction::InitializeSuperclass {
                steps,
                selector,
                argument_registers,
                destination,
            } => {
                self.u8(35);
                self.u16(*steps);
                self.symbol(*selector);
                self.registers(argument_registers);
                self.u16(*destination);
            }
            Instruction::InitializeAppliedLevel {
                steps,
                argument_registers,
                destination,
            } => {
                self.u8(36);
                self.u16(*steps);
                self.registers(argument_registers);
                self.u16(*destination);
            }
        }
    }

    fn literal(&mut self, literal: &Literal) {
        match literal {
            Literal::Nil => self.u8(0),
            Literal::Boolean(value) => {
                self.u8(1);
                self.boolean(*value);
            }
            Literal::SmallInteger(value) => {
                self.u8(2);
                self.u64(*value as u64);
            }
            Literal::Float(value) => {
                self.u8(3);
                self.u64(value.to_bits());
            }
            Literal::String(text) => {
                self.u8(4);
                self.str(text);
            }
            Literal::Symbol(symbol) => {
                self.u8(5);
                self.symbol(*symbol);
            }
            Literal::LargeInteger(integer) => {
                self.u8(6);
                self.boolean(integer.is_negative());
                let digits = integer.magnitude();
                self.length(digits.len());
                for digit in digits {
                    self.u64(*digit);
                }
            }
            Literal::UnresolvedIdentifier(name) => {
                self.u8(7);
                self.str(name);
            }
            Literal::BodyTooLarge => self.u8(8),
        }
    }
}
