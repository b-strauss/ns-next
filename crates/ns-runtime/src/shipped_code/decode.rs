//! The artifact's bytes back into compiled mixins, field for field as
//! `encode.rs` writes them. A tag that names nothing panics: the bytes are
//! the binary's own.

use std::rc::Rc;

use ns_syntax::ast::AccessModifier;
use ns_syntax::Span;

use crate::builtins::primitive::MirrorPrimitive;
use crate::bytecode::{
    CallSite, CodeBuilder, CodeKind, CompiledCode, Instruction, IntegerOperation, Literal,
    UnwindMarker,
};
use crate::mixin::{
    AccessorKind, ClassAccessor, ClauseMessage, DeclarationText, Factory, FactoryMethod,
    MethodEntry, MethodFastPath, Mixin, MixinApplicationClause, MixinMethod, MixinSlot,
    QuickConstant, SlotAccessor,
};
use crate::resolver::RootShape;
use crate::types::BuiltinClass;
use crate::ArbitraryInteger;
use crate::Symbol;

use super::bytes::ByteReader;

/// The root mixin of one module's section, every mixin of it sharing
/// `module_text`, the module's text, as its declaration's
/// (`DeclarationText::shared`).
pub(crate) fn decode_section(section: &[u8], module_text: Rc<str>) -> Rc<Mixin> {
    let mut reader = ByteReader::new(section);
    let symbols = (0..reader.length())
        .map(|_| Symbol::intern(reader.str()))
        .collect();
    let mut decoder = Decoder {
        reader,
        symbols,
        module_text,
    };
    let root = decoder.mixin_fields(true);
    let root = Rc::new(root);
    Mixin::set_enclosing_of_children(&root);
    root
}

struct Decoder<'bytes> {
    reader: ByteReader<'bytes>,
    symbols: Vec<Symbol>,
    /// The module's text, which every mixin decoded shares: a shipped
    /// mixin is never replaced, so no version of it outlives the text.
    module_text: Rc<str>,
}

impl Decoder<'_> {
    fn symbol(&mut self) -> Symbol {
        let index = usize::try_from(self.reader.u32()).expect("internal: a u32 fits a usize");
        *self
            .symbols
            .get(index)
            .unwrap_or_else(|| panic!("internal: the embedded shipped code has symbol {index}"))
    }

    fn option<T>(&mut self, read: impl FnOnce(&mut Self) -> T) -> Option<T> {
        self.reader.boolean().then(|| read(self))
    }

    fn access(&mut self) -> AccessModifier {
        match self.reader.u8() {
            0 => AccessModifier::Public,
            1 => AccessModifier::Protected,
            2 => AccessModifier::Private,
            tag => panic!("internal: the embedded shipped code has access tag {tag}"),
        }
    }

    fn clause_message(&mut self) -> ClauseMessage {
        let selector = self.symbol();
        let has_arguments = self.reader.boolean();
        ClauseMessage {
            selector,
            has_arguments,
        }
    }

    fn span(&mut self) -> Span {
        let start = self.reader.u32();
        let end = self.reader.u32();
        Span::new(start, end)
    }

    /// A nested declaration's mixin, shared and linked to its own.
    fn mixin(&mut self) -> Rc<Mixin> {
        let mixin = Rc::new(self.mixin_fields(false));
        Mixin::set_enclosing_of_children(&mixin);
        mixin
    }

    /// A mixin with its text, before it is shared: its class side, decoded
    /// before its span, takes the text it shares only once the span is
    /// read. `root` says it is the module's root, whose text is the whole
    /// module.
    fn mixin_fields(&mut self, root: bool) -> Mixin {
        let mut mixin = Mixin::empty(self.symbol());
        mixin.access = self.access();
        for _ in 0..self.reader.length() {
            let selector = self.symbol();
            let entry = self.method_entry();
            mixin.methods.insert(selector, entry);
        }
        mixin.slots = (0..self.reader.length())
            .map(|_| {
                let name = self.symbol();
                let mutable = self.reader.boolean();
                let access = self.access();
                let lazy = self.reader.boolean();
                MixinSlot {
                    name,
                    mutable,
                    access,
                    lazy,
                    declaration: None,
                }
            })
            .collect();
        mixin.factory = self.option(|decoder| {
            let selector = decoder.symbol();
            let argument_count = decoder.reader.length();
            Factory {
                selector,
                argument_count,
            }
        });
        mixin.superclass = self.option(Decoder::clause_message);
        mixin.superclass_receiver = self.option(Decoder::symbol);
        mixin.mixin_applications = (0..self.reader.length())
            .map(|_| {
                let message = self.clause_message();
                let receiver = self.option(Decoder::symbol);
                let message_code = self.option(Decoder::code);
                MixinApplicationClause {
                    message,
                    receiver,
                    message_code,
                }
            })
            .collect();
        mixin.has_initializer_expressions = self.reader.boolean();
        mixin.nested = (0..self.reader.length()).map(|_| self.mixin()).collect();
        let class_side = self.option(|decoder| decoder.mixin_fields(false));
        mixin.span = self.span();
        mixin.initializer_code = self.option(Decoder::code);
        mixin.superclass_clause_code = self.option(Decoder::code);
        mixin.superclass_message_code = self.option(Decoder::code);
        mixin.root_shape = self.option(|decoder| {
            let over_value = decoder.reader.boolean();
            let applies_value = decoder.reader.boolean();
            RootShape {
                over_value,
                applies_value,
            }
        });
        mixin.shipped = self.reader.boolean();
        mixin.text = self.declaration_text(&mixin, root);
        mixin.class_side = class_side.map(|mut class_side| {
            class_side.text = mixin.text.clone();
            let class_side = Rc::new(class_side);
            Mixin::set_enclosing_of_children(&class_side);
            class_side
        });
        mixin
    }

    /// `mixin`'s text over the module's: the whole module for its root,
    /// its span otherwise, its nested mixins' spans cut out.
    fn declaration_text(&self, mixin: &Mixin, root: bool) -> Option<Rc<DeclarationText>> {
        let range = if root {
            Span::new(
                0,
                u32::try_from(self.module_text.len()).expect("internal: a shipped module fits"),
            )
        } else {
            mixin.span
        };
        let holes: Vec<Span> = mixin.nested.iter().map(|nested| nested.span).collect();
        let text = DeclarationText::shared(Rc::clone(&self.module_text), range, &holes);
        assert!(
            text.is_some(),
            "internal: the embedded shipped code's spans fit its module's text"
        );
        text.map(Rc::new)
    }

    fn method_entry(&mut self) -> MethodEntry {
        match self.reader.u8() {
            0 => {
                let selector = self.symbol();
                let access = self.access();
                let argument_count = self.reader.length();
                let code = self.code();
                let fast_path = self.option(Decoder::fast_path);
                let class_accessor = self.option(|decoder| {
                    let depth = decoder.reader.u16();
                    let class_name = decoder.symbol();
                    ClassAccessor { depth, class_name }
                });
                MethodEntry::Compiled(Rc::new(MixinMethod {
                    selector,
                    access,
                    argument_count,
                    declaration: None,
                    code,
                    fast_path,
                    class_accessor,
                }))
            }
            1 => {
                let selector = self.symbol();
                let access = self.access();
                let own_index = self.reader.u32();
                let kind = match self.reader.u8() {
                    0 => AccessorKind::Getter,
                    1 => AccessorKind::Setter,
                    tag => panic!("internal: the embedded shipped code has accessor tag {tag}"),
                };
                MethodEntry::SlotAccessor(SlotAccessor {
                    selector,
                    access,
                    own_index,
                    kind,
                })
            }
            2 => {
                let selector = self.symbol();
                let argument_count = self.reader.length();
                let access = self.access();
                MethodEntry::Factory(FactoryMethod {
                    selector,
                    argument_count,
                    access,
                })
            }
            tag => panic!("internal: the embedded shipped code has method entry tag {tag}"),
        }
    }

    fn fast_path(&mut self) -> MethodFastPath {
        match self.reader.u8() {
            0 => MethodFastPath::Mirror(self.mirror_primitive()),
            1 => {
                let position = self.reader.u8();
                MethodFastPath::BuiltinClass(
                    *BuiltinClass::ALL
                        .get(usize::from(position))
                        .unwrap_or_else(|| {
                            panic!("internal: the embedded shipped code has builtin {position}")
                        }),
                )
            }
            2 => MethodFastPath::Constant(match self.reader.u8() {
                0 => QuickConstant::True,
                1 => QuickConstant::False,
                2 => QuickConstant::Nil,
                tag => panic!("internal: the embedded shipped code has constant tag {tag}"),
            }),
            3 => MethodFastPath::Receiver,
            4 => MethodFastPath::Parameter(self.reader.u8()),
            5 => MethodFastPath::ValueOfParameter(self.reader.u8()),
            6 => MethodFastPath::CullParameter(self.reader.u8()),
            tag => panic!("internal: the embedded shipped code has fast path tag {tag}"),
        }
    }

    fn mirror_primitive(&mut self) -> MirrorPrimitive {
        MirrorPrimitive::from_index(self.reader.u16())
    }

    fn code_kind(&mut self) -> CodeKind {
        match self.reader.u8() {
            0 => CodeKind::Method,
            1 => CodeKind::Block,
            2 => CodeKind::Initializer,
            3 => CodeKind::SuperclassClause,
            4 => CodeKind::SuperclassMessage,
            tag => panic!("internal: the embedded shipped code has code kind tag {tag}"),
        }
    }

    fn code(&mut self) -> Rc<CompiledCode> {
        let kind = self.code_kind();
        let argument_count = self.reader.u16();
        let register_count = self.reader.u16();
        let copied_count = self.reader.u16();
        let statement_program_counter = (0..self.reader.length())
            .map(|_| {
                let statement = self.reader.length();
                let start = self.reader.length();
                let end = self.reader.length();
                (statement, start..end)
            })
            .collect();
        let unwind_marker = self.option(Decoder::unwind_marker);
        let source = self.span();
        let mut builder = CodeBuilder::new();
        for _ in 0..self.reader.length() {
            let instruction = self.instruction();
            builder.emit(instruction);
        }
        let mut code = CompiledCode::from_builder(
            kind,
            builder,
            argument_count,
            register_count,
            copied_count,
            statement_program_counter,
            source,
        );
        code.unwind_marker = unwind_marker;
        Rc::new(code)
    }

    fn unwind_marker(&mut self) -> UnwindMarker {
        match self.reader.u8() {
            0 => {
                let class_register = self.reader.u16();
                let block_register = self.reader.u16();
                let active_register = self.reader.u16();
                UnwindMarker::Handler {
                    class_register,
                    block_register,
                    active_register,
                }
            }
            1 => {
                let block_register = self.reader.u16();
                let complete_register = self.reader.u16();
                UnwindMarker::Ensure {
                    block_register,
                    complete_register,
                }
            }
            2 => {
                let block_register = self.reader.u16();
                let complete_register = self.reader.u16();
                UnwindMarker::IfCurtailed {
                    block_register,
                    complete_register,
                }
            }
            tag => panic!("internal: the embedded shipped code has unwind marker tag {tag}"),
        }
    }

    fn registers(&mut self) -> Vec<u16> {
        (0..self.reader.length())
            .map(|_| self.reader.u16())
            .collect()
    }

    fn call_site(&mut self) -> CallSite {
        CallSite {
            number: self.reader.u32(),
        }
    }

    fn instruction(&mut self) -> Instruction {
        match self.reader.u8() {
            0 => {
                let destination = self.reader.u16();
                let literal = self.literal();
                Instruction::LoadLiteral {
                    destination,
                    literal,
                }
            }
            1 => Instruction::LoadSelf {
                destination: self.reader.u16(),
            },
            2 => {
                let destination = self.reader.u16();
                let depth = self.reader.u16();
                Instruction::LoadEnclosingObject { destination, depth }
            }
            3 => {
                let destination = self.reader.u16();
                let source = self.reader.u16();
                Instruction::Move {
                    destination,
                    source,
                }
            }
            4 => {
                let source = self.reader.u16();
                let slot = self.reader.u16();
                Instruction::StoreLocal { source, slot }
            }
            5 => {
                let destination = self.reader.u16();
                let depth = self.reader.u16();
                let index = self.reader.u16();
                Instruction::LoadOuterLocal {
                    destination,
                    depth,
                    index,
                }
            }
            6 => {
                let source = self.reader.u16();
                let depth = self.reader.u16();
                let index = self.reader.u16();
                Instruction::StoreOuterLocal {
                    source,
                    depth,
                    index,
                }
            }
            7 => {
                let depth = self.reader.u16();
                let selector = self.symbol();
                let argument_registers = self.registers();
                let destination = self.reader.u16();
                let call_site = self.call_site();
                Instruction::SendLexical {
                    depth,
                    selector,
                    argument_registers,
                    destination,
                    call_site,
                }
            }
            8 => {
                let selector = self.symbol();
                let receiver_register = self.reader.u16();
                let argument_registers = self.registers();
                let destination = self.reader.u16();
                let call_site = self.call_site();
                Instruction::SendPublic {
                    selector,
                    receiver_register,
                    argument_registers,
                    destination,
                    call_site,
                }
            }
            9 => {
                let selector = self.symbol();
                let receiver_register = self.reader.u16();
                let argument_registers = self.registers();
                let destination = self.reader.u16();
                let call_site = self.call_site();
                Instruction::SendInlinable {
                    selector,
                    receiver_register,
                    argument_registers,
                    destination,
                    call_site,
                }
            }
            10 => {
                let selector = self.symbol();
                let argument_registers = self.registers();
                let destination = self.reader.u16();
                let call_site = self.call_site();
                Instruction::SendSuper {
                    selector,
                    argument_registers,
                    destination,
                    call_site,
                }
            }
            11 => {
                let primitive = self.mirror_primitive();
                let mirror_register = self.reader.u16();
                let subject_register = self.reader.u16();
                let argument_registers = self.registers();
                let destination = self.reader.u16();
                let failure = self.reader.u32();
                Instruction::SendMirror {
                    primitive,
                    mirror_register,
                    subject_register,
                    argument_registers,
                    destination,
                    failure,
                }
            }
            12 => {
                let destination = self.reader.u16();
                let parameters = (0..self.reader.length()).map(|_| self.symbol()).collect();
                let code = self.code();
                let copied = self.registers();
                Instruction::MakeClosure {
                    destination,
                    parameters: Rc::new(parameters),
                    code,
                    copied,
                }
            }
            13 => {
                let destination = self.reader.u16();
                let element_registers = self.registers();
                Instruction::MakeArray {
                    destination,
                    element_registers,
                }
            }
            14 => {
                let destination = self.reader.u16();
                let size = self.reader.u16();
                Instruction::MakeVector { destination, size }
            }
            15 => {
                let destination = self.reader.u16();
                let vector = self.reader.u16();
                let index = self.reader.u16();
                Instruction::LoadVector {
                    destination,
                    vector,
                    index,
                }
            }
            16 => {
                let vector = self.reader.u16();
                let index = self.reader.u16();
                let source = self.reader.u16();
                Instruction::StoreVector {
                    vector,
                    index,
                    source,
                }
            }
            17 => {
                let source = self.reader.u16();
                let own_index = self.reader.u32();
                Instruction::InitializeSlot { source, own_index }
            }
            18 => Instruction::ReturnSlotIfFilled {
                own_index: self.reader.u32(),
            },
            19 => {
                let destination = self.reader.u16();
                let closure = self.reader.u16();
                Instruction::MakeFuture {
                    destination,
                    closure,
                }
            }
            20 => {
                let destination = self.reader.u16();
                let source = self.reader.u16();
                Instruction::ResolveFuture {
                    destination,
                    source,
                }
            }
            21 => {
                let destination = self.reader.u16();
                let own_index = self.reader.u32();
                Instruction::LoadOwnSlot {
                    destination,
                    own_index,
                }
            }
            22 => {
                let source = self.reader.u16();
                let own_index = self.reader.u32();
                Instruction::InitializeSlotProvisionally { source, own_index }
            }
            23 => Instruction::Return {
                register: self.reader.u16(),
            },
            24 => Instruction::ImplicitReturn {
                register: self.reader.u16(),
            },
            25 => Instruction::NonLocalReturn {
                register: self.reader.u16(),
            },
            26 => Instruction::SignalNonBoolean {
                register: self.reader.u16(),
            },
            27 => Instruction::Jump {
                target: self.reader.u32(),
            },
            28 => {
                let register = self.reader.u16();
                let target = self.reader.u32();
                Instruction::JumpIfNotBoolean { register, target }
            }
            29 => {
                let register = self.reader.u16();
                let target = self.reader.u32();
                Instruction::BranchIfFalse { register, target }
            }
            30 => {
                let register = self.reader.u16();
                let target = self.reader.u32();
                Instruction::BranchIfTrue { register, target }
            }
            31 => {
                let operation = match self.reader.u8() {
                    0 => IntegerOperation::Add,
                    1 => IntegerOperation::Subtract,
                    2 => IntegerOperation::Multiply,
                    3 => IntegerOperation::FloorDivide,
                    4 => IntegerOperation::Less,
                    5 => IntegerOperation::Greater,
                    6 => IntegerOperation::LessOrEqual,
                    7 => IntegerOperation::GreaterOrEqual,
                    8 => IntegerOperation::Equal,
                    tag => panic!("internal: the embedded shipped code has operation tag {tag}"),
                };
                let destination = self.reader.u16();
                let left = self.reader.u16();
                let right = self.reader.u16();
                let target = self.reader.u32();
                Instruction::IntegerBinary {
                    operation,
                    destination,
                    left,
                    right,
                    target,
                }
            }
            tag => panic!("internal: the embedded shipped code has instruction tag {tag}"),
        }
    }

    fn literal(&mut self) -> Literal {
        match self.reader.u8() {
            0 => Literal::Nil,
            1 => Literal::Boolean(self.reader.boolean()),
            2 => Literal::SmallInteger(self.reader.u64() as i64),
            3 => Literal::Float(f64::from_bits(self.reader.u64())),
            4 => Literal::String(self.reader.str().to_owned()),
            5 => Literal::Symbol(self.symbol()),
            6 => {
                let negative = self.reader.boolean();
                let digits: Vec<u64> = (0..self.reader.length())
                    .map(|_| self.reader.u64())
                    .collect();
                Literal::LargeInteger(ArbitraryInteger::from_parts(negative, digits))
            }
            7 => Literal::UnresolvedIdentifier(self.reader.str().to_owned()),
            8 => Literal::BodyTooLarge,
            tag => panic!("internal: the embedded shipped code has literal tag {tag}"),
        }
    }
}
