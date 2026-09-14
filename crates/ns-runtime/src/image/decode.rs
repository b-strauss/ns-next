//! Canonical decoding, the inverse of `encode.rs`. A malformed byte stream
//! is a `DecodeError`, never a panic.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_syntax::Span;

use crate::bytecode::{CallSite, CodeKind, CompiledCode, Instruction, Literal};
use crate::heap::Heap;
use crate::interpreter::instantiate;
use crate::types::{
    BuiltinClass, ClassReference, ClosureData, Frame, FrameKind, FrameReference, Object,
    ObjectData, ObjectReference, SourceOrigin, Value,
};
use crate::Mutator;
use crate::Symbol;

use super::opcodes::*;
use super::{mixins, symbols};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    UnexpectedEndOfFile,
    BadMagic,
    VersionMismatch {
        found: u32,
        expected: u32,
    },
    InvalidTag {
        what: &'static str,
        tag: u8,
    },
    /// The header set a flag bit this build assigns no meaning to. Each
    /// flag governs whether a section is present, so an unknown one means
    /// the reader cannot know where the next section starts -- it would
    /// decode whatever follows as the wrong thing rather than fail.
    UnknownHeaderFlags {
        flags: u8,
    },
    /// A frame's `program_counter` is not an instruction index of the body that
    /// runs in it. The VM stores the index of the instruction it is executing,
    /// so every real activation's `program_counter` is in range; a crafted one
    /// out of range would be an unchecked index the moment anything resumed,
    /// resumed-through, or reported the frame.
    ProgramCounterOutOfRange {
        program_counter: usize,
        instructions: usize,
    },
    InvalidUtf8,
    InvalidSpan {
        start: u32,
        end: u32,
    },
    /// A recursive section nested past its decoding ceiling. Refusing is
    /// the only option: recursing on to a stack overflow aborts the
    /// process and cannot be reported at all.
    NestingTooDeep {
        what: &'static str,
        limit: usize,
    },
    /// A body in the code table creates blocks nested deeper than the
    /// parser lets source nest them (`MAX_BLOCK_NESTING`). The table is flat
    /// on the wire, so nothing recurses while it is read; the depth matters
    /// afterwards, twice over. The depth count exempts a block a
    /// control-flow native activates for a `SendInlinable` when the
    /// activation below created it, so the exempt activations stacked above
    /// one counted activation are bounded by exactly this nesting
    /// (`docs/design/compiler.md`, "How the limit counts") -- unbounded, the
    /// depth limit would bound nothing. And a decoded body owns the bodies
    /// it creates, so dropping one recurses once per level.
    BlockNestingTooDeep {
        limit: usize,
    },
    BadCodeId(u32),
    BadObjectId(u32),
    BadFrameId(u32),
    /// A mixin-table row, or a class object's record, referenced a row id
    /// past the end of the table.
    BadMixinId(u32),
    /// A field the format types as a class reference -- an object's own
    /// `class`, a class object's `superclass` or `declaring_class`, a
    /// frame's `current_class` or the class its kind names -- resolved to an
    /// object-table record that is not a class object. Nothing in the byte
    /// stream forces a tag-2 class tag to name a *class* record, so only a
    /// check after the whole table is read can tell. Left unchecked it is
    /// silent wrong behaviour rather than a crash: `class_of` would hand
    /// dispatch an object with no application, and `outer` would walk a
    /// chain that is not a declaration chain.
    NotAClassReference,
    /// A closure frame's kind names an object that is not a closure.
    NotAClosureReference,
    /// A class object's own `class` is another record in the image's object
    /// table. `Mutator::new_class_object` always gives a class object
    /// the shared bootstrap `Object` placeholder, so no real world contains
    /// one -- and `Object`'s iterative `Drop` cannot hand the `class` field
    /// to its worklist, so a chain of them is the one link left that would
    /// still release recursively. Refusing keeps it one link long.
    ClassOfAClassIsARecord,
    /// A class object's enclosing object decoded to a scalar. §6 makes the
    /// enclosing object the nested class's only state, and it is always the
    /// receiver the accessor ran on -- an object; a scalar would leave
    /// `dispatch::kth_enclosing_object` with nothing to hand an `outer`
    /// send.
    EnclosingNotAnObject,
    /// The image's class objects' superclass links form a cycle, so no
    /// order exists in which their applications could be rebuilt (each
    /// needs its superclass's). Unreachable from a real world -- a
    /// superclass is fixed before the class object that inherits it exists
    /// -- and refusing is what keeps the rebuild a finite walk.
    SuperclassCycle,
    /// The mixin table's rows are not `mixin::preorder` of row 0: a row is
    /// named as a child by two rows or by none, a row is its own ancestor,
    /// or the children are named out of the order the walk visits them in.
    /// Row position is the whole correspondence between a row and the
    /// declaration it describes, so a table that does not reproduce that
    /// walk is refused outright rather than paired up wrongly.
    MixinTableNotATree {
        row: u32,
    },
    /// One mixin-table row named the same selector twice, which would leave
    /// its rebuilt method dictionary smaller than its own code list.
    DuplicateSelector(String),
    /// A selector or name reference (a send's selector, a method-dictionary
    /// key, a closure's parameter name, a `Symbol` object's text, ...) that
    /// indexes past the end of the image's symbol table. Every such
    /// reference is a `u32` id checked against the table at the point of
    /// use, never trusted to have been validated elsewhere.
    BadSymbolId(u32),
    /// A closure record's parameter list and the code it names disagree about
    /// how many arguments the closure takes. The two are separate fields on the
    /// wire, so only a decode-time reconciliation can catch a crafted mismatch
    /// -- and it must, because the two halves are read by different parts of
    /// `Mutator::push_step`: it checks the *call's* arity against
    /// `parameters.len()` but sizes the register file from the *code*
    /// (`register_count - argument_count`). Disagreement leaves
    /// `locals::read_local` computing `temporaries[i - arguments.len()]` past
    /// the end of `temporaries` -- an unchecked index panic from a
    /// version-matching image -- or, the other way round, silently
    /// misaddressing temporaries as arguments.
    ClosureArityMismatch {
        parameters: usize,
        argument_count: u16,
    },
    /// The image's mixin table is empty, so it describes no root class --
    /// and a world without one has nothing for the graph's class tag 1 to
    /// name and no declaration to instantiate.
    NotAClass,
    /// A selector the image names has no declaration to bind to: a
    /// mixin-table row's code list naming a selector its own method
    /// dictionary lacks, or a `FrameKind::Method` naming a selector its
    /// `current_class` does not declare.
    UnknownSelector(String),
    /// A mixin-table row declares slots or `initExpr`s but carries no
    /// compiled initializer. Instantiating such a class runs initializer
    /// code on every `X new`, and `Mutator::initializer_body` panics on that
    /// invariant rather than failing -- so only refusing here keeps a
    /// crafted image from aborting the process at the first `X new`.
    MissingInitializer,
    /// A mixin-table row declares a superclass clause the image carries no
    /// compiled clause for. The nested-class accessor runs that code on the
    /// first access to the declaration, and panics on the invariant when
    /// the mixin carries none -- the same reason `MissingInitializer` is a
    /// refusal rather than a deferred crash.
    MissingSuperclassClause,
    /// A mixin-table row's superclass clause carries a factory message with
    /// arguments the image has no compiled body for. `instantiate`
    /// evaluates that body on every instantiation of the class, and panics
    /// on the invariant when the mixin carries none -- the same refusal, for
    /// the same reason, as `MissingSuperclassClause`.
    MissingSuperclassMessage,
    /// A compiled body attached where a body of another kind belongs, or
    /// named as a closure's body when it is not a block body. The kinds are
    /// not interchangeable: a block body runs in a `FrameKind::Closure`
    /// activation and a method's or initializer's does not, which is what
    /// `frame_at_depth` (and so `verify_code_table`'s outer-local bound)
    /// walks.
    WrongCodeKind {
        expected: &'static str,
    },
    /// A method's -- or the instance initializer's -- compiled body takes a
    /// different number of arguments than its own declaration does. The frame's
    /// `arguments` come from the send (or the factory), its `temporaries` from
    /// the body's `argument_count`; a disagreement leaves `read_local` indexing
    /// `temporaries` past its end.
    CodeArityMismatch {
        what: String,
        parameters: usize,
        argument_count: u16,
    },
    /// A body with no instructions, or one whose last instruction falls
    /// through: the VM fetches `instructions[program_counter]` after every
    /// fall-through, so either runs off the end of the vector.
    UnterminatedBody,
    /// A register operand outside its own body's register file.
    RegisterOutOfRange {
        register: u16,
        register_count: u16,
    },
    /// An outer-local operand outside the register file of the body it
    /// addresses -- the one `depth` closure hops out.
    OuterLocalOutOfRange {
        depth: u16,
        index: u16,
        register_count: u16,
    },
    /// A jump target that is not an instruction index of its own body.
    JumpTargetOutOfRange {
        target: u32,
        instructions: usize,
    },
    /// More arguments than registers: an activation is pushed with
    /// `register_count - argument_count` temporaries, which underflows here.
    ArgumentsExceedRegisters {
        argument_count: u16,
        register_count: u16,
    },
    /// A `MakeClosure` (or a closure record) naming a body that is not in
    /// the code table it is verified against.
    CodeNotInTable,
    /// One block body created by more than one `MakeClosure`. The compiler
    /// wraps every block body in its own `Rc`, used exactly once, which is
    /// what makes "the body one closure hop out" well defined.
    SharedBlockBody,
    /// A decoded frame's register file does not match the body that runs in
    /// it, so a register index the body considers in range would index
    /// `arguments`/`temporaries` out of range.
    FrameRegisterFile {
        arguments: usize,
        locals: usize,
        argument_count: u16,
        register_count: u16,
    },
    /// A decoded closure's captured frame is not an activation of the body
    /// its own body is lexically nested in -- the link every
    /// `LoadOuterLocal`/`StoreOuterLocal` bound rests on.
    CapturedFrameMismatch,
    /// An object's identity hash is `u32::MAX`, so there is no hash a
    /// loaded world could allocate *next*:
    /// `Mutator::advance_identity_hash_past` would have to compute
    /// `u32::MAX + 1` (an overflow panic in this workspace's dev profile,
    /// and a wrap to 0 -- colliding with the loaded objects it exists to
    /// avoid -- in release), and even saturating it would only move the
    /// overflow to the counter's own `+= 1`. Refusing the image
    /// is the only answer that neither panics nor collides.
    IdentityHashExhausted,
    /// A tag-named hash of 0. 0 means "not yet assigned", which the encoder
    /// expresses by leaving the object out; accepting a 0 would give one world
    /// two encodings.
    UnassignedTagNamedHash,
    /// One tag-named object listed twice in the closing hash table.
    DuplicateTagNamedHash,
    /// Bytes remain after the last section. Refused rather than ignored: with
    /// two artifact shapes in one format, silently accepting a tail means a
    /// binary with a graph appended -- or an image truncated to a binary and
    /// then padded -- loads as something its author did not write.
    TrailingBytes {
        count: usize,
    },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::UnexpectedEndOfFile => write!(f, "unexpected end of image"),
            DecodeError::BadMagic => write!(f, "not an ns-next image (bad magic)"),
            DecodeError::VersionMismatch { found, expected } => {
                write!(f, "image format version {found} is not supported (this build reads version {expected})")
            }
            DecodeError::InvalidTag { what, tag } => write!(f, "invalid {what} tag {tag}"),
            DecodeError::UnknownHeaderFlags { flags } => write!(
                f,
                "image header sets flag bits {:#04x} this build does not know",
                flags & !super::encode::KNOWN_FLAGS
            ),
            DecodeError::ProgramCounterOutOfRange { program_counter, instructions } => write!(
                f,
                "frame pc {program_counter} is not an instruction index in a body of {instructions} instruction(s)"
            ),
            DecodeError::InvalidUtf8 => write!(f, "invalid UTF-8 in image string"),
            DecodeError::InvalidSpan { start, end } => {
                write!(f, "source span {start}..{end} ends before it starts")
            }
            DecodeError::NestingTooDeep { what, limit } => {
                write!(f, "{what} in image nests deeper than {limit} levels")
            }
            DecodeError::BlockNestingTooDeep { limit } => {
                write!(f, "blocks in image code nest deeper than {limit} levels")
            }
            DecodeError::BadCodeId(id) => write!(f, "code id {id} out of range"),
            DecodeError::BadObjectId(id) => write!(f, "object id {id} out of range"),
            DecodeError::BadFrameId(id) => write!(f, "frame id {id} out of range"),
            DecodeError::BadSymbolId(id) => write!(f, "symbol id {id} out of range"),
            DecodeError::BadMixinId(id) => write!(f, "mixin id {id} out of range"),
            DecodeError::NotAClassReference => write!(
                f,
                "an image field that must name a class object names an ordinary object instead"
            ),
            DecodeError::NotAClosureReference => write!(
                f,
                "a closure frame in the image names an object that is not a closure"
            ),
            DecodeError::ClassOfAClassIsARecord => write!(
                f,
                "a class object in the image is itself an instance of another class object in the \
                 image; every class object's own class is the bootstrap `Object` placeholder"
            ),
            DecodeError::EnclosingNotAnObject => write!(
                f,
                "a class object's enclosing object is a scalar, not an object"
            ),
            DecodeError::SuperclassCycle => write!(
                f,
                "the image's class objects inherit from each other in a cycle, so no application \
                 chain could be rebuilt for them"
            ),
            DecodeError::MixinTableNotATree { row } => write!(
                f,
                "the image's mixin table is not a declaration tree: row {row} is reached out of \
                 declaration order, twice, or not at all"
            ),
            DecodeError::DuplicateSelector(selector) => write!(
                f,
                "a mixin-table row names method `{selector}` more than once"
            ),
            DecodeError::ClosureArityMismatch { parameters, argument_count } => write!(
                f,
                "closure record declares {parameters} parameters but the code it names takes {argument_count}"
            ),
            DecodeError::NotAClass => {
                write!(f, "the image's mixin table describes no root class")
            }
            DecodeError::UnknownSelector(selector) => write!(
                f,
                "the image names method `{selector}`, which the declaration it belongs to does not define"
            ),
            DecodeError::MissingInitializer => write!(
                f,
                "a class in the image needs an instance initializer, which the image has no compiled code for"
            ),
            DecodeError::MissingSuperclassClause => write!(
                f,
                "the image declares a superclass clause it has no compiled code for"
            ),
            DecodeError::MissingSuperclassMessage => write!(
                f,
                "the image's superclass clause passes factory arguments it has no compiled code for"
            ),
            DecodeError::WrongCodeKind { expected } => {
                write!(f, "image code is not a {expected} body")
            }
            DecodeError::CodeArityMismatch {
                what,
                parameters,
                argument_count,
            } => write!(
                f,
                "the image's compiled {what} takes {argument_count} argument(s), but its declaration says {parameters}"
            ),
            DecodeError::UnterminatedBody => write!(
                f,
                "image code body does not end in a return or a jump, so it would run past its last instruction"
            ),
            DecodeError::RegisterOutOfRange {
                register,
                register_count,
            } => write!(
                f,
                "register r{register} is outside a body with {register_count} register(s)"
            ),
            DecodeError::OuterLocalOutOfRange {
                depth,
                index,
                register_count,
            } => write!(
                f,
                "outer local r{index} at depth {depth} is outside the enclosing body's {register_count} register(s)"
            ),
            DecodeError::JumpTargetOutOfRange { target, instructions } => write!(
                f,
                "jump target {target} is not an instruction index in a body of {instructions} instruction(s)"
            ),
            DecodeError::ArgumentsExceedRegisters {
                argument_count,
                register_count,
            } => write!(
                f,
                "image code takes {argument_count} argument(s) but declares only {register_count} register(s)"
            ),
            DecodeError::CodeNotInTable => {
                write!(f, "image code refers to a body outside its code table")
            }
            DecodeError::SharedBlockBody => write!(
                f,
                "one block body in the image is created by more than one MakeClosure"
            ),
            DecodeError::FrameRegisterFile {
                arguments,
                locals,
                argument_count,
                register_count,
            } => write!(
                f,
                "a decoded frame holds {arguments} argument(s) and {locals} local(s), but the code that runs in it takes {argument_count} argument(s) in {register_count} register(s)"
            ),
            DecodeError::CapturedFrameMismatch => write!(
                f,
                "a decoded closure's captured frame does not run the body its own body is nested in"
            ),
            DecodeError::IdentityHashExhausted => write!(
                f,
                "an image object's identity hash is {}, leaving no hash for a loaded world to allocate next",
                u32::MAX
            ),
            DecodeError::UnassignedTagNamedHash => write!(
                f,
                "an image records identity hash 0 for an object it names by tag; an unassigned hash is recorded by leaving the object out"
            ),
            DecodeError::DuplicateTagNamedHash => write!(
                f,
                "an image records the identity hash of one tag-named object twice"
            ),
            DecodeError::TrailingBytes { count } => {
                write!(f, "{count} trailing byte(s) after the last section")
            }
        }
    }
}

pub struct Reader<'a> {
    buffer: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buffer: &'a [u8]) -> Reader<'a> {
        Reader {
            buffer,
            position: 0,
        }
    }

    /// Bytes not yet consumed. Every item of a counted section costs at
    /// least one byte, so this bounds how many can really follow -- see
    /// `reserve_for`.
    pub fn remaining(&self) -> usize {
        self.buffer.len() - self.position
    }

    /// Bytes already consumed -- the offset of whatever comes next. Lets a
    /// caller that decoded a known-shape prefix (the header, the source, a
    /// fixed section) locate exactly where the next section starts, e.g. to
    /// corrupt one field of it in a hostile-input test without hardcoding
    /// byte offsets that would drift as the format's earlier fields change.
    pub fn position(&self) -> usize {
        self.position
    }

    pub fn read_raw(&mut self, length: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(DecodeError::UnexpectedEndOfFile)?;
        if end > self.buffer.len() {
            return Err(DecodeError::UnexpectedEndOfFile);
        }
        let slice = &self.buffer[self.position..end];
        self.position = end;
        Ok(slice)
    }

    pub fn read_u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.read_raw(1)?[0])
    }

    pub fn read_boolean(&mut self) -> Result<bool, DecodeError> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            tag => Err(DecodeError::InvalidTag { what: "bool", tag }),
        }
    }

    pub fn read_u16(&mut self) -> Result<u16, DecodeError> {
        let bytes: [u8; 2] = self
            .read_raw(2)?
            .try_into()
            .expect("read_raw(2) returns 2 bytes");
        Ok(u16::from_le_bytes(bytes))
    }

    pub fn read_u32(&mut self) -> Result<u32, DecodeError> {
        let bytes: [u8; 4] = self
            .read_raw(4)?
            .try_into()
            .expect("read_raw(4) returns 4 bytes");
        Ok(u32::from_le_bytes(bytes))
    }

    pub fn read_i64(&mut self) -> Result<i64, DecodeError> {
        let bytes: [u8; 8] = self
            .read_raw(8)?
            .try_into()
            .expect("read_raw(8) returns 8 bytes");
        Ok(i64::from_le_bytes(bytes))
    }

    pub fn read_f64(&mut self) -> Result<f64, DecodeError> {
        let bytes: [u8; 8] = self
            .read_raw(8)?
            .try_into()
            .expect("read_raw(8) returns 8 bytes");
        Ok(f64::from_le_bytes(bytes))
    }

    pub fn read_bytes(&mut self) -> Result<Vec<u8>, DecodeError> {
        let length = self.read_u32()? as usize;
        Ok(self.read_raw(length)?.to_vec())
    }

    pub fn read_str(&mut self) -> Result<String, DecodeError> {
        String::from_utf8(self.read_bytes()?).map_err(|_| DecodeError::InvalidUtf8)
    }
}

/// What the header says about the rest of the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Whether a source blob follows the header -- the debug variant. The
    /// source is never executed and never re-parsed; it is carried so a
    /// loaded world can be shown, and eventually exported, as `.ns`.
    pub has_source: bool,
    /// No graph section follows; the file ends after the entry field.
    pub stateless: bool,
}

pub fn read_header(reader: &mut Reader) -> Result<Header, DecodeError> {
    let magic = reader.read_raw(4)?;
    if magic != super::encode::MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let version = reader.read_u32()?;
    if version != super::encode::FORMAT_VERSION {
        return Err(DecodeError::VersionMismatch {
            found: version,
            expected: super::encode::FORMAT_VERSION,
        });
    }
    let flags = reader.read_u8()?;
    // A flag this build does not implement means a section it cannot see:
    // continuing would read the next field out of whatever that section
    // occupies. Refused rather than masked off.
    if flags & !super::encode::KNOWN_FLAGS != 0 {
        return Err(DecodeError::UnknownHeaderFlags { flags });
    }
    Ok(Header {
        has_source: flags & super::encode::FLAG_HAS_SOURCE != 0,
        stateless: flags & super::encode::FLAG_STATELESS != 0,
    })
}

/// The inverse of `encode::encode_entry`: a presence byte and, when set, the
/// symbol id it names, resolved against the image's own symbol table so an
/// out-of-range id is a `DecodeError` rather than an index panic.
pub fn decode_entry(
    reader: &mut Reader,
    symbols: &symbols::Table,
) -> Result<Option<Symbol>, DecodeError> {
    match reader.read_u8()? {
        0 => Ok(None),
        1 => Ok(Some(symbols.get(reader.read_u32()?)?)),
        tag => Err(DecodeError::InvalidTag { what: "entry", tag }),
    }
}

/// The inverse of `encode::encode_source_origin`: the provenance a loaded
/// world reports as its own.
pub fn decode_source_origin(reader: &mut Reader) -> Result<SourceOrigin, DecodeError> {
    match reader.read_u8()? {
        0 => Ok(SourceOrigin::File(PathBuf::from(reader.read_str()?))),
        tag => Err(DecodeError::InvalidTag {
            what: "source origin",
            tag,
        }),
    }
}

pub fn decode_scalar_value<'gc>(reader: &mut Reader) -> Result<Value<'gc>, DecodeError> {
    match reader.read_u8()? {
        0 => Ok(Value::Nil),
        1 => Ok(Value::Boolean(false)),
        2 => Ok(Value::Boolean(true)),
        3 => Ok(Value::SmallInteger(reader.read_i64()?)),
        4 => Ok(Value::Float(reader.read_f64()?)),
        tag => Err(DecodeError::InvalidTag { what: "value", tag }),
    }
}

pub fn builtin_class_from_tag(tag: u8) -> Option<BuiltinClass> {
    Some(match tag {
        0 => BuiltinClass::Object,
        1 => BuiltinClass::Boolean,
        2 => BuiltinClass::True,
        3 => BuiltinClass::False,
        4 => BuiltinClass::Integer,
        5 => BuiltinClass::Float,
        6 => BuiltinClass::String,
        7 => BuiltinClass::Symbol,
        8 => BuiltinClass::Character,
        9 => BuiltinClass::Array,
        10 => BuiltinClass::OrderedCollection,
        11 => BuiltinClass::Nil,
        12 => BuiltinClass::Block,
        13 => BuiltinClass::Platform,
        _ => return None,
    })
}

pub fn read_builtin_class(reader: &mut Reader) -> Result<BuiltinClass, DecodeError> {
    let tag = reader.read_u8()?;
    builtin_class_from_tag(tag).ok_or(DecodeError::InvalidTag {
        what: "builtin class",
        tag,
    })
}

/// The decoded code table: every compiled body the image carries, flat and
/// in the order it was written. Which body belongs to which declaration is
/// the mixin table's business (`image::mixins`), which names these by index.
#[derive(Debug, PartialEq)]
pub struct DecodedCodeTable {
    pub codes: Vec<Rc<CompiledCode>>,
}

pub fn decode_code_table(
    reader: &mut Reader,
    symbols: &symbols::Table,
) -> Result<DecodedCodeTable, DecodeError> {
    let count = reader.read_u32()? as usize;
    let mut codes: Vec<Rc<CompiledCode>> = reserve_for(count, reader);
    // How deep the blocks under each body already read nest, keyed like
    // `code_index`.
    let mut nesting: HashMap<usize, usize> = HashMap::new();
    for _ in 0..count {
        let code = Rc::new(decode_code(reader, &codes, symbols)?);
        let depth = block_nesting(&code, &nesting)?;
        nesting.insert(Rc::as_ptr(&code) as usize, depth);
        codes.push(code);
    }
    Ok(DecodedCodeTable { codes })
}

/// `ns-syntax`'s own nesting ceiling, which bounds how deeply a program's
/// blocks nest: `parser::MAX_DEPTH` in `crates/ns-syntax/src/parser/mod.rs`,
/// 128, private to that crate and so restated here, as `MAX_LITERAL_DEPTH`
/// restates it. Every block literal is one `parse_primary`, one level of
/// that count, and neither lowering compiles a block into anything but one
/// `MakeClosure` of one body -- the optimized lowering only inlines some away
/// -- so no program the parser accepts nests deeper than this in a code
/// table. A lower ceiling would refuse to load a binary built from a legal
/// program; `the_deepest_block_nesting_the_parser_accepts_builds_loads_and_runs`
/// in `tests/image.rs` probes the parser's real limit and fails if it moves
/// past this one.
///
/// It is what makes the depth limit bound memory for an artifact as it does
/// for source: the exempt activations above one counted activation are
/// bounded by the static block nesting (`DecodeError::BlockNestingTooDeep`).
const MAX_BLOCK_NESTING: usize = 128;

/// How many `MakeClosure` hops the deepest block `code` creates sits below
/// it, from the depths of the bodies it creates -- already decoded, since a
/// block precedes the code that creates it, so one pass over the table
/// measures every body.
///
/// Refused here, as each body is read, rather than by `verify_code_table`
/// afterwards: `code` owns the bodies it creates through their `Rc`s, so a
/// chain of them is dropped by a recursion as deep as the chain. Measured
/// on the way in, no chain longer than the ceiling is ever built, and
/// dropping a refused table stays shallow. Accepted whole, the 100,000-deep
/// chain of `a_deeply_nested_block_chain_is_a_decode_error_not_a_stack_overflow`
/// (`tests/image.rs`) overflows the stack when it is dropped.
fn block_nesting(
    code: &CompiledCode,
    nesting: &HashMap<usize, usize>,
) -> Result<usize, DecodeError> {
    let mut depth = 0;
    for instruction in &code.instructions {
        if let Instruction::MakeClosure { code: block, .. } = instruction {
            // `decode_instruction` resolved the id against the bodies already
            // read, and every one of them was measured.
            let below = nesting
                .get(&(Rc::as_ptr(block) as usize))
                .ok_or(DecodeError::CodeNotInTable)?;
            depth = depth.max(below + 1);
        }
    }
    if depth > MAX_BLOCK_NESTING {
        return Err(DecodeError::BlockNestingTooDeep {
            limit: MAX_BLOCK_NESTING,
        });
    }
    Ok(depth)
}

/// `Vec::with_capacity` for a count read from an untrusted stream: every
/// item costs at least one byte, so the bytes still unread are an upper
/// bound on how many can really follow. Without the clamp a corrupt count
/// prefix would reserve gigabytes and abort the process before the read
/// could fail with `UnexpectedEndOfFile`.
pub(crate) fn reserve_for<T>(count: usize, reader: &Reader) -> Vec<T> {
    Vec::with_capacity(count.min(reader.remaining()))
}

fn check_code_id(id: u32, codes: &[Rc<CompiledCode>]) -> Result<(), DecodeError> {
    if (id as usize) < codes.len() {
        Ok(())
    } else {
        Err(DecodeError::BadCodeId(id))
    }
}

/// Reconciles every decoded mixin with the bodies its row attached.
///
/// Both halves of a row come off the wire -- a declaration's arity and kind on
/// one side, a code-table id on the other -- so a crafted image can make them
/// disagree:
///
/// - a body's `kind` and `argument_count` against the declaration's own arity
///   (`check_code_arity`), because the activation a body runs in is built
///   from the *declaration* -- `arguments` from the send or the factory --
///   while its temporaries are sized from the *body*;
/// - the presence of a body wherever a declaration makes one mandatory (the
///   three `Missing*` errors), because the run-time paths that reach for
///   those bodies panic on the invariant rather than failing.
///
/// Methods are checked in selector order, the order the encoder writes a row
/// in, so for any artifact the encoder produced the reported error is the
/// row's first bad entry. `read_row` enforces no order, so for a crafted row
/// that lists its selectors otherwise it is merely one of them.
///
/// `mixins::decode` attaches the bodies but checks none of this, so `inspect`
/// can still display an artifact whose halves disagree; `image::load` calls
/// this before anything is built on the code.
pub fn verify_mixins(mixins: &mixins::Decoded) -> Result<(), DecodeError> {
    for mixin in &mixins.mixins {
        let mut methods: Vec<&Rc<crate::mixin::MixinMethod>> = mixin.methods.values().collect();
        methods.sort_by(|left, right| left.selector.as_str().cmp(right.selector.as_str()));
        for method in methods {
            check_code_arity(
                &method.code,
                CodeKind::Method,
                format!("method `{}`", method.selector),
                method.argument_count,
            )?;
        }
        match &mixin.initializer_code {
            // The same reconciliation for the initializer, whose
            // `argument_count` is the mixin's factory parameter count and whose
            // activation's `arguments` are the factory arguments `instantiate`
            // checked against that same pattern.
            Some(code) => check_code_arity(
                code,
                CodeKind::Initializer,
                "instance initializer".to_string(),
                mixin.factory.map_or(0, |factory| factory.argument_count),
            )?,
            // A class with slots or `initExpr`s runs compiled initializer
            // code on every `X new`, and `Mutator::initializer_body` *panics*
            // when the mixin carries none. The row's declaration half says
            // the mixin needs one and its code half says whether the image
            // carries it -- two fields only this function compares.
            None if instantiate::mixin_needs_initializer_code(mixin) => {
                return Err(DecodeError::MissingInitializer)
            }
            None => {}
        }
        match &mixin.superclass_clause_code {
            // A clause body takes no arguments: it is evaluated in an
            // activation the *accessor* builds on the enclosing object, not
            // in one carrying the nested class's factory arguments.
            Some(code) => check_code_arity(
                code,
                CodeKind::SuperclassClause,
                "superclass clause".to_string(),
                0,
            )?,
            // The same reconciliation as the initializer's: a mixin that
            // declares a superclass clause has its accessor run that
            // clause's code on the first nested-class access, and
            // `interpreter::nested` panics when the mixin carries none.
            None if mixin.superclass.is_some() => return Err(DecodeError::MissingSuperclassClause),
            None => {}
        }
        match &mixin.superclass_message_code {
            // Unlike the clause's receiver, the clause's *message* arguments
            // run in an activation carrying this mixin's own factory
            // arguments (`newspeak-spec.pdf` §6.2), so their body's arity is
            // the factory pattern's -- the same reconciliation the instance
            // initializer above gets, for the same reason.
            Some(code) => check_code_arity(
                code,
                CodeKind::SuperclassMessage,
                "superclass clause message".to_string(),
                mixin.factory.map_or(0, |factory| factory.argument_count),
            )?,
            // A mixin whose clause message carries arguments evaluates them
            // on every instantiation, and
            // `instantiate::clause_message_body` panics when the mixin
            // carries no body for them.
            None if mixin_needs_superclass_message_code(mixin) => {
                return Err(DecodeError::MissingSuperclassMessage)
            }
            None => {}
        }
    }
    Ok(())
}

/// Whether `mixin`'s superclass clause carries a message with arguments, and
/// therefore needs a compiled body for them. Mirrors
/// `instantiate::mixin_needs_initializer_code`: the one question
/// `verify_mixins` asks of a decoded declaration, answered off the same
/// field `instantiate::clause_message_body` decides by at run time.
fn mixin_needs_superclass_message_code(mixin: &Rc<crate::mixin::Mixin>) -> bool {
    mixin.superclass.is_some_and(|clause| clause.has_arguments)
}

/// One decoded body against the declaration it is attached to:
/// the same kind, and the same number of arguments.
fn check_code_arity(
    code: &CompiledCode,
    kind: CodeKind,
    what: String,
    parameters: usize,
) -> Result<(), DecodeError> {
    if code.kind != kind {
        return Err(DecodeError::WrongCodeKind {
            expected: match kind {
                CodeKind::Method => "method",
                CodeKind::Block => "block",
                CodeKind::Initializer => "initializer",
                CodeKind::SuperclassClause => "superclass clause",
                CodeKind::SuperclassMessage => "superclass clause message",
            },
        });
    }
    if code.argument_count as usize != parameters {
        return Err(DecodeError::CodeArityMismatch {
            what,
            parameters,
            argument_count: code.argument_count,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Structural verification of a decoded code table.
//
// Decoding is byte-safe: every id, tag and length is checked as it is read. It
// does not make the result *coherent*. The VM indexes
// `code.instructions[program_counter]` directly, and
// `locals::read_local`/`write_local` index a frame's `arguments`/`temporaries`
// directly, so a jump target, a register operand or an arity that decoded
// cleanly can still panic the process the first time the body runs. Everything
// below is a bounded, linear pass over the flat, topologically ordered table --
// no traversal, no recursion -- and reports a `DecodeError` rather than leaving
// the VM to trip over it.
// ---------------------------------------------------------------------------

/// `Rc` pointer -> code-table index, the inverse of `encode::CodeIds`.
fn code_index(table: &DecodedCodeTable) -> HashMap<usize, usize> {
    table
        .codes
        .iter()
        .enumerate()
        .map(|(index, code)| (Rc::as_ptr(code) as usize, index))
        .collect()
}

/// For each body, the body whose `MakeClosure` creates it -- the lexical
/// parent, and so (the VM's `MakeClosure` arm sets
/// `captured_frame` to the creating activation) the activation one
/// `LoadOuterLocal` hop out. `None` for a method or initializer body, and
/// for a block body nothing creates.
///
/// Checks every `MakeClosure` on the way, because each is what makes the
/// relation single-valued and meaningful: the named body must be in this
/// table, must be a block body, must agree with the instruction's own
/// parameter list about its arity, and must have exactly one creator.
fn lexical_parents(table: &DecodedCodeTable) -> Result<Vec<Option<usize>>, DecodeError> {
    let index_of = code_index(table);
    let mut parents: Vec<Option<usize>> = vec![None; table.codes.len()];
    for (index, code) in table.codes.iter().enumerate() {
        for instruction in &code.instructions {
            let Instruction::MakeClosure {
                parameters,
                code: block,
                ..
            } = instruction
            else {
                continue;
            };
            let id = *index_of
                .get(&(Rc::as_ptr(block) as usize))
                .ok_or(DecodeError::CodeNotInTable)?;
            if block.kind != CodeKind::Block {
                return Err(DecodeError::WrongCodeKind { expected: "block" });
            }
            // The `MakeClosure` mirror of `decode_closure_record`'s check:
            // `push_step` validates the call against `parameters.len()`
            // but sizes the register file from the body's `argument_count`.
            if parameters.len() != block.argument_count as usize {
                return Err(DecodeError::ClosureArityMismatch {
                    parameters: parameters.len(),
                    argument_count: block.argument_count,
                });
            }
            if parents[id].is_some() {
                return Err(DecodeError::SharedBlockBody);
            }
            parents[id] = Some(index);
        }
    }
    Ok(parents)
}

/// Every decoded body checked against what the VM will do with it:
/// its arity fits its register file, it cannot run past its last
/// instruction, every jump target is one of its own instructions, every
/// register operand is inside its own register file, and every outer local
/// is inside the register file of the body it addresses.
pub fn verify_code_table(table: &DecodedCodeTable) -> Result<(), DecodeError> {
    let parents = lexical_parents(table)?;
    for (index, code) in table.codes.iter().enumerate() {
        verify_body(code)?;
        verify_outer_locals(table, &parents, index, code)?;
    }
    Ok(())
}

fn verify_body(code: &CompiledCode) -> Result<(), DecodeError> {
    // Every activation is pushed with `register_count - argument_count`
    // temporaries, a `u16` subtraction.
    if code.argument_count > code.register_count {
        return Err(DecodeError::ArgumentsExceedRegisters {
            argument_count: code.argument_count,
            register_count: code.register_count,
        });
    }
    // The VM fetches `instructions[program_counter]` at the top of the loop and
    // again after every fall-through, so an empty body -- or one whose last
    // instruction falls through -- indexes past the end of the vector.
    match code.instructions.last() {
        Some(
            Instruction::Return { .. }
            | Instruction::ImplicitReturn { .. }
            | Instruction::NonLocalReturn { .. }
            | Instruction::Jump { .. },
        ) => {}
        _ => return Err(DecodeError::UnterminatedBody),
    }
    for instruction in &code.instructions {
        for register in own_registers(instruction) {
            if register >= code.register_count {
                return Err(DecodeError::RegisterOutOfRange {
                    register,
                    register_count: code.register_count,
                });
            }
        }
        if let Some(target) = jump_target(instruction) {
            if target as usize >= code.instructions.len() {
                return Err(DecodeError::JumpTargetOutOfRange {
                    target,
                    instructions: code.instructions.len(),
                });
            }
        }
    }
    Ok(())
}

/// Every operand that addresses a register of the *executing* frame.
/// `LoadOuterLocal`/`StoreOuterLocal`'s `index` is deliberately absent: it
/// addresses another frame's register file, and `verify_outer_locals`
/// bounds it against that one.
fn own_registers(instruction: &Instruction) -> Vec<u16> {
    match instruction {
        Instruction::LoadLiteral { destination, .. }
        | Instruction::LoadSelf { destination }
        | Instruction::LoadBuiltinClass { destination, .. }
        | Instruction::MakeClosure { destination, .. } => vec![*destination],
        Instruction::Move {
            destination,
            source,
        } => vec![*destination, *source],
        // `slot` is a register index too: the VM writes it with
        // `write_local`, exactly as it does a `Move`'s `destination`.
        Instruction::StoreLocal { source, slot } => vec![*source, *slot],
        Instruction::LoadOuterLocal { destination, .. } => vec![*destination],
        Instruction::StoreOuterLocal { source, .. } => vec![*source],
        Instruction::SendLexical {
            argument_registers,
            destination,
            ..
        }
        | Instruction::SendSuper {
            argument_registers,
            destination,
            ..
        } => {
            let mut registers = argument_registers.clone();
            registers.push(*destination);
            registers
        }
        Instruction::SendPublic {
            receiver_register,
            argument_registers,
            destination,
            ..
        }
        | Instruction::SendInlinable {
            receiver_register,
            argument_registers,
            destination,
            ..
        } => {
            let mut registers = argument_registers.clone();
            registers.push(*receiver_register);
            registers.push(*destination);
            registers
        }
        Instruction::MakeArray {
            destination,
            element_registers,
        } => {
            let mut registers = element_registers.clone();
            registers.push(*destination);
            registers
        }
        Instruction::InitializeSlot { source, .. } => vec![*source],
        Instruction::Return { register }
        | Instruction::ImplicitReturn { register }
        | Instruction::NonLocalReturn { register }
        | Instruction::JumpIfNotBoolean { register, .. }
        | Instruction::BranchIfFalse { register, .. }
        | Instruction::BranchIfTrue { register, .. } => vec![*register],
        Instruction::Jump { .. } => vec![],
    }
}

/// Exhaustive, like `own_registers` and for the same reason: a new
/// jump-carrying `Instruction` variant must fail to compile here rather than
/// slip through a wildcard and reach the VM's `program_counter` unchecked.
fn jump_target(instruction: &Instruction) -> Option<u32> {
    match instruction {
        Instruction::Jump { target }
        | Instruction::JumpIfNotBoolean { target, .. }
        | Instruction::BranchIfFalse { target, .. }
        | Instruction::BranchIfTrue { target, .. } => Some(*target),
        Instruction::LoadLiteral { .. }
        | Instruction::LoadSelf { .. }
        | Instruction::LoadBuiltinClass { .. }
        | Instruction::Move { .. }
        | Instruction::StoreLocal { .. }
        | Instruction::LoadOuterLocal { .. }
        | Instruction::StoreOuterLocal { .. }
        | Instruction::SendLexical { .. }
        | Instruction::SendPublic { .. }
        | Instruction::SendInlinable { .. }
        | Instruction::SendSuper { .. }
        | Instruction::MakeClosure { .. }
        | Instruction::MakeArray { .. }
        | Instruction::InitializeSlot { .. }
        | Instruction::Return { .. }
        | Instruction::ImplicitReturn { .. }
        | Instruction::NonLocalReturn { .. } => None,
    }
}

fn verify_outer_locals(
    table: &DecodedCodeTable,
    parents: &[Option<usize>],
    index: usize,
    code: &CompiledCode,
) -> Result<(), DecodeError> {
    for instruction in &code.instructions {
        let (depth, local) = match instruction {
            Instruction::LoadOuterLocal { depth, index, .. }
            | Instruction::StoreOuterLocal { depth, index, .. } => (*depth, *index),
            _ => continue,
        };
        let enclosing = &table.codes[enclosing_body(table, parents, index, depth)];
        if local >= enclosing.register_count {
            return Err(DecodeError::OuterLocalOutOfRange {
                depth,
                index: local,
                register_count: enclosing.register_count,
            });
        }
    }
    Ok(())
}

/// `locals::frame_at_depth`'s static counterpart: the body of the activation
/// `depth` hops out from an activation of `table.codes[at]`.
///
/// The two agree because `verify_graph` and the VM between them
/// keep every closure's `captured_frame` an activation of its body's
/// lexical parent -- created that way at run time, checked on load -- and
/// because only a block body ever runs in a `FrameKind::Closure`
/// activation (`lexical_parents` and `verify_graph` both refuse any other
/// kind as a closure body). `frame_at_depth` stops hopping at a
/// non-closure activation, so a depth past the lexical nesting lands on
/// the outermost body rather than running off the chain; a block body no
/// `MakeClosure` creates stops there too, and stopping is the conservative
/// answer -- no activation of it can exist for `verify_graph` to admit.
fn enclosing_body(
    table: &DecodedCodeTable,
    parents: &[Option<usize>],
    mut body_index: usize,
    depth: u16,
) -> usize {
    for _ in 0..depth {
        if table.codes[body_index].kind != CodeKind::Block {
            break;
        }
        match parents[body_index] {
            Some(parent) => body_index = parent,
            None => break,
        }
    }
    body_index
}

fn decode_code(
    reader: &mut Reader,
    codes: &[Rc<CompiledCode>],
    symbols: &symbols::Table,
) -> Result<CompiledCode, DecodeError> {
    let kind = match reader.read_u8()? {
        0 => CodeKind::Method,
        1 => CodeKind::Block,
        2 => CodeKind::Initializer,
        3 => CodeKind::SuperclassClause,
        4 => CodeKind::SuperclassMessage,
        tag => {
            return Err(DecodeError::InvalidTag {
                what: "code kind",
                tag,
            })
        }
    };
    let argument_count = reader.read_u16()?;
    let register_count = reader.read_u16()?;
    let start = reader.read_u32()?;
    let end = reader.read_u32()?;
    if start > end {
        return Err(DecodeError::InvalidSpan { start, end });
    }
    let source = Span::new(start, end);
    let program_counter_count = reader.read_u32()? as usize;
    let mut statement_program_counter = reserve_for(program_counter_count, reader);
    for _ in 0..program_counter_count {
        let statement = reader.read_u32()? as usize;
        let range_start = reader.read_u32()? as usize;
        let range_end = reader.read_u32()? as usize;
        statement_program_counter.push((statement, range_start..range_end));
    }
    let site_count = reader.read_u32()? as usize;
    let mut call_sites = reserve_for(site_count, reader);
    for _ in 0..site_count {
        call_sites.push(decode_call_site(reader)?);
    }
    let instruction_count = reader.read_u32()? as usize;
    let mut instructions = reserve_for(instruction_count, reader);
    for _ in 0..instruction_count {
        instructions.push(decode_instruction(reader, codes, symbols)?);
    }
    Ok(CompiledCode {
        kind,
        instructions,
        argument_count,
        register_count,
        statement_program_counter,
        call_sites,
        source,
    })
}

fn decode_call_site(reader: &mut Reader) -> Result<CallSite, DecodeError> {
    Ok(CallSite {
        call_site_id: reader.read_u16()?,
    })
}

fn decode_registers(reader: &mut Reader) -> Result<Vec<u16>, DecodeError> {
    let count = reader.read_u16()? as usize;
    let mut registers = reserve_for(count, reader);
    for _ in 0..count {
        registers.push(reader.read_u16()?);
    }
    Ok(registers)
}

/// The table lookup that replaces a per-occurrence `Symbol::intern`: every
/// selector or name in the code table is a `u32` id into the symbol table
/// decoded once, up front, in `image::load`.
fn decode_symbol(reader: &mut Reader, symbols: &symbols::Table) -> Result<Symbol, DecodeError> {
    symbols.get(reader.read_u32()?)
}

fn decode_instruction(
    reader: &mut Reader,
    codes: &[Rc<CompiledCode>],
    symbols: &symbols::Table,
) -> Result<Instruction, DecodeError> {
    Ok(match reader.read_u8()? {
        OPCODE_LOAD_LITERAL => {
            let destination = reader.read_u16()?;
            Instruction::LoadLiteral {
                destination,
                literal: decode_literal(reader, 0, symbols)?,
            }
        }
        OPCODE_LOAD_SELF => Instruction::LoadSelf {
            destination: reader.read_u16()?,
        },
        OPCODE_LOAD_BUILTIN_CLASS => {
            let destination = reader.read_u16()?;
            Instruction::LoadBuiltinClass {
                destination,
                kind: read_builtin_class(reader)?,
            }
        }
        OPCODE_MOVE => {
            let destination = reader.read_u16()?;
            Instruction::Move {
                destination,
                source: reader.read_u16()?,
            }
        }
        OPCODE_STORE_LOCAL => {
            let source = reader.read_u16()?;
            Instruction::StoreLocal {
                source,
                slot: reader.read_u16()?,
            }
        }
        OPCODE_LOAD_OUTER_LOCAL => {
            let destination = reader.read_u16()?;
            let depth = reader.read_u16()?;
            Instruction::LoadOuterLocal {
                destination,
                depth,
                index: reader.read_u16()?,
            }
        }
        OPCODE_STORE_OUTER_LOCAL => {
            let source = reader.read_u16()?;
            let depth = reader.read_u16()?;
            Instruction::StoreOuterLocal {
                source,
                depth,
                index: reader.read_u16()?,
            }
        }
        OPCODE_SEND_LEXICAL => {
            let depth = reader.read_u16()?;
            let selector = decode_symbol(reader, symbols)?;
            let argument_registers = decode_registers(reader)?;
            let destination = reader.read_u16()?;
            Instruction::SendLexical {
                depth,
                selector,
                argument_registers,
                destination,
                call_site: decode_call_site(reader)?,
            }
        }
        OPCODE_SEND_PUBLIC => {
            let selector = decode_symbol(reader, symbols)?;
            let receiver_register = reader.read_u16()?;
            let argument_registers = decode_registers(reader)?;
            let destination = reader.read_u16()?;
            Instruction::SendPublic {
                selector,
                receiver_register,
                argument_registers,
                destination,
                call_site: decode_call_site(reader)?,
            }
        }
        OPCODE_SEND_INLINABLE => {
            let selector = decode_symbol(reader, symbols)?;
            let receiver_register = reader.read_u16()?;
            let argument_registers = decode_registers(reader)?;
            let destination = reader.read_u16()?;
            Instruction::SendInlinable {
                selector,
                receiver_register,
                argument_registers,
                destination,
                call_site: decode_call_site(reader)?,
            }
        }
        OPCODE_SEND_SUPER => {
            let selector = decode_symbol(reader, symbols)?;
            let argument_registers = decode_registers(reader)?;
            let destination = reader.read_u16()?;
            Instruction::SendSuper {
                selector,
                argument_registers,
                destination,
                call_site: decode_call_site(reader)?,
            }
        }
        OPCODE_MAKE_CLOSURE => {
            let destination = reader.read_u16()?;
            let count = reader.read_u16()? as usize;
            let mut parameters = reserve_for(count, reader);
            for _ in 0..count {
                parameters.push(decode_symbol(reader, symbols)?);
            }
            let id = reader.read_u32()?;
            // Post-order guarantees a block's code precedes its parent's,
            // so `codes` already holds it -- a forward or out-of-range id
            // is corruption.
            check_code_id(id, codes)?;
            Instruction::MakeClosure {
                destination,
                parameters: Rc::new(parameters),
                code: codes[id as usize].clone(),
            }
        }
        OPCODE_MAKE_ARRAY => {
            let destination = reader.read_u16()?;
            Instruction::MakeArray {
                destination,
                element_registers: decode_registers(reader)?,
            }
        }
        OPCODE_INITIALIZE_SLOT => {
            let source = reader.read_u16()?;
            Instruction::InitializeSlot {
                source,
                own_index: reader.read_u32()?,
            }
        }
        OPCODE_RETURN => Instruction::Return {
            register: reader.read_u16()?,
        },
        OPCODE_IMPLICIT_RETURN => Instruction::ImplicitReturn {
            register: reader.read_u16()?,
        },
        OPCODE_NON_LOCAL_RETURN => Instruction::NonLocalReturn {
            register: reader.read_u16()?,
        },
        OPCODE_JUMP => Instruction::Jump {
            target: reader.read_u32()?,
        },
        OPCODE_JUMP_IF_NOT_BOOLEAN => {
            let register = reader.read_u16()?;
            Instruction::JumpIfNotBoolean {
                register,
                target: reader.read_u32()?,
            }
        }
        OPCODE_BRANCH_IF_FALSE => {
            let register = reader.read_u16()?;
            Instruction::BranchIfFalse {
                register,
                target: reader.read_u32()?,
            }
        }
        OPCODE_BRANCH_IF_TRUE => {
            let register = reader.read_u16()?;
            Instruction::BranchIfTrue {
                register,
                target: reader.read_u32()?,
            }
        }
        tag => {
            return Err(DecodeError::InvalidTag {
                what: "instruction",
                tag,
            })
        }
    })
}

/// How deeply a `Literal::Array` may nest before decoding refuses. Each
/// level costs only five bytes in the stream (`u8` tag + `u32` length), so
/// without a ceiling a few kilobytes of crafted image would recurse until
/// the stack overflows -- which aborts the process outright and cannot be
/// caught, let alone reported as a `DecodeError`.
///
/// The value is bracketed from both sides. Below, by what the encoder can
/// legitimately produce: `ns-syntax`'s parser accepts literal-array nesting
/// up to its own `parser::MAX_DEPTH` (128, shared across every guarded
/// production, and spent on nested arrays by `parse_array_element`), and
/// `compiler::lower::lower_literal` lowers all of it to nested
/// `Literal::Array`s with no further limit. So this ceiling must be *at
/// least* `MAX_DEPTH`, or an image would fail to read back a program the
/// compiler had just written into it. Above, by the stack: decoding was
/// measured to survive 700 levels and to overflow at 2000 in a debug build,
/// so 128 keeps a wide margin.
///
/// `parser::MAX_DEPTH` is private to `ns-syntax`, and runtime work leaves
/// that crate untouched, so the number is restated here rather than
/// imported -- it must track `crates/ns-syntax/src/parser/mod.rs`'s
/// `MAX_DEPTH`. `the_deepest_literal_array_the_parser_accepts_survives_a_round_trip`
/// in `tests/image.rs` discovers the parser's real bound at run time instead
/// of assuming it, so the two cannot drift apart silently.
const MAX_LITERAL_DEPTH: usize = 128;

fn decode_literal(
    reader: &mut Reader,
    depth: usize,
    symbols: &symbols::Table,
) -> Result<Literal, DecodeError> {
    if depth > MAX_LITERAL_DEPTH {
        return Err(DecodeError::NestingTooDeep {
            what: "literal array",
            limit: MAX_LITERAL_DEPTH,
        });
    }
    Ok(match reader.read_u8()? {
        LITERAL_NIL => Literal::Nil,
        LITERAL_BOOLEAN => Literal::Boolean(reader.read_boolean()?),
        LITERAL_SMALL_INTEGER => Literal::SmallInteger(reader.read_i64()?),
        LITERAL_FLOAT => Literal::Float(reader.read_f64()?),
        LITERAL_STRING => Literal::String(reader.read_str()?),
        LITERAL_SYMBOL => Literal::Symbol(decode_symbol(reader, symbols)?),
        LITERAL_CHARACTER => {
            let code = reader.read_u32()?;
            Literal::Character(char::from_u32(code).ok_or(DecodeError::InvalidTag {
                what: "char",
                tag: 6,
            })?)
        }
        LITERAL_ARRAY => {
            let count = reader.read_u32()? as usize;
            let mut elements = reserve_for(count, reader);
            for _ in 0..count {
                elements.push(decode_literal(reader, depth + 1, symbols)?);
            }
            Literal::Array(elements)
        }
        LITERAL_MALFORMED_INTEGER => Literal::MalformedInteger(reader.read_str()?),
        LITERAL_UNRESOLVED_IDENTIFIER => Literal::UnresolvedIdentifier(reader.read_str()?),
        tag => {
            return Err(DecodeError::InvalidTag {
                what: "literal",
                tag,
            })
        }
    })
}

// ---------------------------------------------------------------------------
// The object table and the frame table.
// ---------------------------------------------------------------------------

/// Everything the graph decoder resolves tags against: the live bootstrap
/// (`mutator`), the root class built from the mixin table, the decoded code
/// table, the interpreter's `platform` singleton, and the decoded
/// declaration tree a class object's mixin id names.
///
/// `mutator` is `&mut` because decoding a class object *allocates into* the
/// interpreter: rebuilding its `MixinApplication` goes through
/// `Mutator::apply_mixin`, and that shared cache is the whole mechanism
/// by which two enclosing instances get back the one application they had
/// before the snapshot.
pub struct DecodeContext<'a, 'gc> {
    pub mutator: &'a mut Mutator<'gc>,
    pub root_class: &'a ClassReference<'gc>,
    pub codes: &'a [Rc<CompiledCode>],
    pub platform: Value<'gc>,
    /// The decoded mixin table's rows, indexed by row id -- the same
    /// `Rc<Mixin>`s that carry the image's compiled bodies and that
    /// `verify_mixins` reconciled, so a class object built around one of
    /// them runs the code the image carries. Using the row's own `Rc` also
    /// matters to the application cache, which keys by `mixin_key`: any
    /// other copy of the same declaration would miss the entry every other
    /// class object of that row shares.
    pub mixins: &'a [Rc<crate::mixin::Mixin>],
}

pub struct DecodedGraph<'gc> {
    pub roots: Vec<Value<'gc>>,
    /// The largest identity hash of any decoded object -- `load` advances
    /// the interpreter's identity-hash counter past it.
    pub max_identity_hash: u32,
    /// The two decoded tables themselves, in image order. The roots reach
    /// most of them, but `verify_graph` must see *every* record -- a frame
    /// no root reaches can still be some closure's captured frame.
    pub objects: Vec<ObjectReference<'gc>>,
    pub frames: Vec<FrameReference<'gc>>,
}

impl<'gc> DecodedGraph<'gc> {
    /// What a stateless binary carries in place of a graph section: nothing.
    /// `load` skips `verify_graph` on this value rather than calling it --
    /// an empty frame list would make `verify_graph` a no-op anyway, but
    /// skipping it says directly that there is nothing to verify.
    pub fn empty() -> Self {
        DecodedGraph {
            roots: Vec::new(),
            max_identity_hash: 0,
            objects: Vec::new(),
            frames: Vec::new(),
        }
    }
}

/// The two placeholder-allocated tables, indexed by the ids in the stream.
pub struct Tables<'a, 'gc> {
    pub objects: &'a [ObjectReference<'gc>],
    pub frames: &'a [FrameReference<'gc>],
}

/// The inverse of `encode::encode_class_tag`. Tag 2's object may still be a
/// placeholder when this runs -- a class object can be named before its own
/// record is read -- so this resolves identity only; that the record really
/// was a class record is `check_class_references`'s job, once every record
/// has been read.
pub fn decode_class_tag<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
) -> Result<ClassReference<'gc>, DecodeError> {
    match reader.read_u8()? {
        CLASS_BUILTIN => Ok(context.mutator.builtin_class(read_builtin_class(reader)?)),
        CLASS_USER_ROOT => Ok(*context.root_class),
        CLASS_RECORD => decode_object_id(reader, tables),
        tag => Err(DecodeError::InvalidTag { what: "class", tag }),
    }
}

fn decode_optional_class<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
) -> Result<Option<ClassReference<'gc>>, DecodeError> {
    if reader.read_boolean()? {
        Ok(Some(decode_class_tag(reader, context, tables)?))
    } else {
        Ok(None)
    }
}

fn decode_value<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
) -> Result<Value<'gc>, DecodeError> {
    match reader.read_u8()? {
        VALUE_NIL => Ok(Value::Nil),
        VALUE_FALSE => Ok(Value::Boolean(false)),
        VALUE_TRUE => Ok(Value::Boolean(true)),
        VALUE_SMALL_INTEGER => Ok(Value::SmallInteger(reader.read_i64()?)),
        VALUE_FLOAT => Ok(Value::Float(reader.read_f64()?)),
        VALUE_OBJECT_ID => Ok(Value::Object(decode_object_id(reader, tables)?)),
        VALUE_CLASS_TAG => Ok(Value::Object(decode_class_tag(reader, context, tables)?)),
        VALUE_PLATFORM => Ok(context.platform),
        tag => Err(DecodeError::InvalidTag { what: "value", tag }),
    }
}

fn decode_values<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
) -> Result<Vec<Value<'gc>>, DecodeError> {
    let count = reader.read_u32()? as usize;
    let mut values = reserve_for(count, reader);
    for _ in 0..count {
        values.push(decode_value(reader, context, tables)?);
    }
    Ok(values)
}

/// An object id off the wire, resolved against the placeholders the first
/// pass allocated: in range, or `BadObjectId` before anything indexes by it.
fn decode_object_id<'gc>(
    reader: &mut Reader,
    tables: &Tables<'_, 'gc>,
) -> Result<ObjectReference<'gc>, DecodeError> {
    let id = reader.read_u32()?;
    tables
        .objects
        .get(id as usize)
        .copied()
        .ok_or(DecodeError::BadObjectId(id))
}

fn decode_frame_id<'gc>(
    reader: &mut Reader,
    tables: &Tables<'_, 'gc>,
) -> Result<FrameReference<'gc>, DecodeError> {
    let id = reader.read_u32()?;
    tables
        .frames
        .get(id as usize)
        .copied()
        .ok_or(DecodeError::BadFrameId(id))
}

fn decode_frame_reference<'gc>(
    reader: &mut Reader,
    tables: &Tables<'_, 'gc>,
) -> Result<Option<FrameReference<'gc>>, DecodeError> {
    if reader.read_boolean()? {
        Ok(Some(decode_frame_id(reader, tables)?))
    } else {
        Ok(None)
    }
}

fn decode_closure_record<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    symbols: &symbols::Table,
) -> Result<ClosureData<'gc>, DecodeError> {
    let count = reader.read_u16()? as usize;
    let mut parameters = reserve_for(count, reader);
    for _ in 0..count {
        parameters.push(decode_symbol(reader, symbols)?);
    }
    let code_id = reader.read_u32()?;
    let code = context
        .codes
        .get(code_id as usize)
        .cloned()
        .ok_or(DecodeError::BadCodeId(code_id))?;
    // The one place in the format that can see both halves of a closure's
    // arity. Checked before the frame ids are read, so a mismatch is
    // reported as itself rather than as whatever the next field trips on.
    if parameters.len() != code.argument_count as usize {
        return Err(DecodeError::ClosureArityMismatch {
            parameters: parameters.len(),
            argument_count: code.argument_count,
        });
    }
    let captured_frame = decode_frame_id(reader, tables)?;
    let home = decode_frame_id(reader, tables)?;
    Ok(ClosureData {
        parameters: Rc::new(parameters),
        code,
        captured_frame,
        home,
    })
}

/// Re-resolves a `FrameKind::Method` against the decoded class's method
/// map: the image names the method by selector, never by identity.
///
/// Against the frame's own `current_class`, which `dispatch::walk` sets to
/// the level whose mixin declares the running method -- so that level's
/// method dictionary is exactly where the selector is found, and the
/// `Rc<MixinMethod>` it yields carries the compiled body this image's row
/// named for it. Now that a nested class object is an
/// ordinary record, `current_class` is regularly a class other than the
/// root, and resolving everything against the root would either miss the
/// selector or bind the frame to a same-named method of a different class.
/// `None` -- a `TopLevel` activation -- has no class to ask, so the root
/// class stands in, as it did for every frame before nesting.
fn method_declaration<'gc>(
    context: &DecodeContext<'_, 'gc>,
    current_class: Option<&ClassReference<'gc>>,
    selector: Symbol,
) -> Result<Rc<crate::mixin::MixinMethod>, DecodeError> {
    let class = current_class.unwrap_or(context.root_class);
    let found = context
        .mutator
        .heap
        .class_application(*class)
        .and_then(|application| application.mixin.methods.get(&selector).cloned());
    found.ok_or(DecodeError::UnknownSelector(selector.as_str().to_string()))
}

fn decode_frame<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    symbols: &symbols::Table,
) -> Result<Frame<'gc>, DecodeError> {
    let receiver = decode_value(reader, context, tables)?;
    let arguments = decode_values(reader, context, tables)?;
    let temporaries = decode_values(reader, context, tables)?;
    let program_counter = reader.read_u32()? as usize;
    let sender = decode_frame_reference(reader, tables)?;
    let home = decode_frame_reference(reader, tables)?;
    let current_class = decode_optional_class(reader, context, tables)?;
    let kind = match reader.read_u8()? {
        FRAME_METHOD => FrameKind::Method(method_declaration(
            context,
            current_class.as_ref(),
            decode_symbol(reader, symbols)?,
        )?),
        FRAME_TOP_LEVEL => FrameKind::TopLevel,
        FRAME_INITIALIZER => FrameKind::Initializer(decode_class_tag(reader, context, tables)?),
        FRAME_SUPERCLASS_CLAUSE => {
            FrameKind::SuperclassClause(decode_class_tag(reader, context, tables)?)
        }
        FRAME_SUPERCLASS_MESSAGE => {
            FrameKind::SuperclassMessage(decode_class_tag(reader, context, tables)?)
        }
        FRAME_CLOSURE_OBJECT => FrameKind::Closure(decode_object_id(reader, tables)?),
        tag => {
            return Err(DecodeError::InvalidTag {
                what: "frame kind",
                tag,
            })
        }
    };
    let uncontinuable = reader.read_boolean()?;
    Ok(Frame {
        receiver,
        arguments,
        temporaries,
        program_counter,
        sender,
        home,
        current_class,
        kind,
        uncontinuable,
        // Nothing re-activates a decoded frame (this module's doc comment,
        // below), so no compiled `InitializeSlot` ever reads this on a frame
        // that came off the wire.
        slot_base: 0,
    })
}

/// A safe, fully initialized stand-in, overwritten in place by
/// `decode_graph_section`'s second pass before anything can observe it.
fn placeholder_frame<'gc>() -> Frame<'gc> {
    Frame {
        receiver: Value::Nil,
        arguments: Vec::new(),
        temporaries: Vec::new(),
        program_counter: 0,
        sender: None,
        home: None,
        current_class: None,
        kind: FrameKind::TopLevel,
        uncontinuable: false,
        slot_base: 0,
    }
}

/// The smallest object record the wire admits: a `UserRoot` class tag (1),
/// an identity hash (4), and a body tag (1). Every body adds at least four
/// more bytes, so this is a floor, not a typical size.
const MIN_OBJECT_RECORD_BYTES: usize = 6;

/// The smallest frame record the wire admits: a `nil` receiver (1), empty
/// `arguments` and `temporaries` counts (4 + 4), a `program_counter` (4),
/// absent `sender`, `home` and `current_class` (1 + 1 + 1), a `TopLevel` kind
/// tag (1), and `uncontinuable` (1). Every other frame kind is larger.
const MIN_FRAME_RECORD_BYTES: usize = 18;

/// The smallest entry of the closing hash table: a tag (1) and a hash (4). A
/// builtin-class entry carries one more byte.
const MIN_TAG_NAMED_HASH_BYTES: usize = 5;

/// A record count read from an untrusted stream, refused before it is used
/// to *allocate*. The bytes still unread bound how many records can really
/// follow: at `bytes_per_record` apiece, a count above
/// `remaining / bytes_per_record` cannot describe what comes next. Without
/// this a crafted `u32::MAX` would allocate billions of placeholders and
/// abort the process before the read could fail.
///
/// Dividing (rather than comparing against `remaining` itself) is what
/// keeps the bound proportional to the real record sizes: at one byte
/// apiece a 100 MB image could still drive ~22 GB of placeholder
/// allocation before the first record failed to read.
pub(crate) fn bounded_count(
    count: usize,
    bytes_per_record: usize,
    reader: &Reader,
) -> Result<usize, DecodeError> {
    if count > reader.remaining() / bytes_per_record {
        return Err(DecodeError::UnexpectedEndOfFile);
    }
    Ok(count)
}

/// The module's root class object, rebuilt from mixin-table row 0 and the
/// root-class section -- `instantiate::load_module` without the source.
///
/// The two do the same three things in the same order, so a loaded world's
/// root class is indistinguishable from a freshly loaded module's, down to
/// its identity hash (both allocate exactly one class object, immediately
/// after the bootstrap's builtins): apply the root mixin over the
/// superclass's own application, then mint the class object with no
/// enclosing object and no declaring class -- the module's root class is
/// the outermost level, so neither exists.
///
/// The superclass arrives as an already-resolved `ClassReference` from
/// `decode_root_superclass`, because `load_module` resolves it by *name*
/// from the declaration and a decoded mixin has no declaration to read a
/// name from.
pub fn build_root_class<'gc>(
    mutator: &mut Mutator<'gc>,
    mixins: &mixins::Decoded,
    superclass: Option<ClassReference<'gc>>,
) -> Result<ClassReference<'gc>, DecodeError> {
    let root_mixin = mixins.root().cloned().ok_or(DecodeError::NotAClass)?;
    let super_application = superclass
        .as_ref()
        .and_then(|superclass| mutator.heap.class_application(*superclass));
    let application = mutator.apply_mixin(&root_mixin, super_application.as_ref());
    Ok(mutator.new_class_object(application, None, None, superclass))
}

/// The root-class section's bytes, read but not resolved: whether a
/// superclass tag follows and, if so, which builtin it names. Shared by
/// `decode_root_superclass`, which resolves the tag against a live
/// interpreter, and `skip_root_superclass`, which only needs to know how
/// many bytes the section spans -- duplicating this reading would let the
/// two drift, and then the span `inspect` records would stop matching the
/// real one.
fn read_root_superclass_tag(reader: &mut Reader) -> Result<Option<BuiltinClass>, DecodeError> {
    if reader.read_boolean()? {
        Ok(Some(read_builtin_class(reader)?))
    } else {
        Ok(None)
    }
}

/// The root-class section: the class the module's root class inherits from,
/// as an optional builtin tag. The inverse of `encode::encode_root_class`,
/// which explains why the format admits only a builtin here.
pub fn decode_root_superclass<'gc>(
    reader: &mut Reader,
    mutator: &Mutator<'gc>,
) -> Result<Option<ClassReference<'gc>>, DecodeError> {
    Ok(read_root_superclass_tag(reader)?.map(|tag| mutator.builtin_class(tag)))
}

/// Reads the root-class section's bytes without resolving the tag to a live
/// class. `inspect` has no interpreter to resolve a builtin tag against --
/// it must not bootstrap one -- and only needs to know where this section
/// ends.
pub fn skip_root_superclass(reader: &mut Reader) -> Result<(), DecodeError> {
    read_root_superclass_tag(reader)?;
    Ok(())
}

/// A class record as read, before its application exists: the object-table
/// slot it fills and the mixin-table row it applies. Its three references
/// are installed in the same pass that reads it (they are ids into tables
/// that are already allocated); only the application has to wait for the
/// superclass's.
struct PendingClass {
    object: usize,
    mixin: Rc<crate::mixin::Mixin>,
}

/// Rebuilds every decoded class object's `MixinApplication`, superclass
/// first.
///
/// An application is derived, never stored: `Mutator::apply_mixin`
/// computes the slot layout and the lookup chain from a mixin and its
/// superclass's application, and caches the result under exactly that pair.
/// So rebuilding restores identity as well as content -- two class objects
/// that shared one application before the snapshot ask for the same pair
/// here and get the same `Rc` back, which is what keeps a call site
/// monomorphic across a save/load.
///
/// The order is the catch: a class object's record may name a superclass
/// whose own record appears later in the table. Each class has at most one
/// superclass, so the dependency structure is a forest; this walks each
/// chain to its already-applied end (a builtin, the root class, or a class
/// resolved by an earlier walk) over an explicit stack, then applies back
/// down it. A chain that closes on itself is `SuperclassCycle` rather than
/// a loop that never ends.
fn rebuild_applications<'gc>(
    context: &mut DecodeContext<'_, 'gc>,
    objects: &[ObjectReference<'gc>],
    pending: Vec<PendingClass>,
) -> Result<(), DecodeError> {
    // Class-object arena index -> position in `pending`, for the "is my
    // superclass still waiting for its own application?" question each step
    // asks. Keyed by index because a superclass arrives as a `ClassReference`
    // handle, not as the id it was written with.
    let mut position: HashMap<usize, usize> = HashMap::with_capacity(pending.len());
    for (index, pending_class) in pending.iter().enumerate() {
        position.insert(
            context
                .mutator
                .heap
                .object_identity_key(objects[pending_class.object]),
            index,
        );
    }
    let mut done = vec![false; pending.len()];
    let mut on_chain = vec![false; pending.len()];
    let mut chain: Vec<usize> = Vec::new();
    for start in 0..pending.len() {
        if done[start] {
            continue;
        }
        let mut current = start;
        loop {
            if on_chain[current] {
                return Err(DecodeError::SuperclassCycle);
            }
            on_chain[current] = true;
            chain.push(current);
            let superclass = context
                .mutator
                .heap
                .class_superclass(objects[pending[current].object]);
            let Some(superclass) = superclass else { break };
            let Some(&next) = position.get(&context.mutator.heap.object_identity_key(superclass))
            else {
                break;
            };
            if done[next] {
                break;
            }
            current = next;
        }
        // Deepest-first: every class in `chain` inherits from the next one,
        // so applying from the far end forward always has the superclass's
        // application in hand.
        for &pending_index in chain.iter().rev() {
            let object = &objects[pending[pending_index].object];
            let super_application = match context.mutator.heap.class_superclass(*object) {
                Some(superclass) => Some(
                    context
                        .mutator
                        .heap
                        .class_application(superclass)
                        .ok_or(DecodeError::NotAClassReference)?,
                ),
                None => None,
            };
            let application = context
                .mutator
                .apply_mixin(&pending[pending_index].mixin, super_application.as_ref());
            if !context
                .mutator
                .heap
                .set_class_application(*object, application)
            {
                unreachable!("this object was filled in as a class record");
            }
            done[pending_index] = true;
            on_chain[pending_index] = false;
        }
        chain.clear();
    }
    Ok(())
}

/// Whether `class` is one of the two class objects a graph never creates:
/// a builtin, or the module's root class. The mirror of
/// `encode::is_class_tag`. The explicit `is_class` guard is redundant with
/// `class.same(root_class)` today -- that can only be true when `class` is
/// literally `root_class`, which is a class by construction -- but it keeps
/// this answering `false` for a non-class object on its own terms rather
/// than through that indirect argument, so it stays honest if either
/// check's meaning shifts later.
fn is_prebuilt_class<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    root_class: ClassReference<'gc>,
) -> bool {
    heap.is_class(class) && (heap.class_builtin(class).is_some() || class.same(root_class))
}

/// Every field the format types as a class reference really names a class
/// object, and a class object's own `class` is one the graph did not
/// create.
///
/// A tag-2 class tag names an object-table id, and nothing in the byte
/// stream forces that record to have been a class record -- which is only
/// knowable once every record has been read, so it is checked here rather
/// than where each tag is decoded. These five positions are the complete
/// set: a class tag appears nowhere else in the graph section, and a plain
/// `Value::Object` pointing at a class object is legitimate (that is what a
/// cached nested class in a slot *is*).
///
/// The extra rule for a class object's *own* `class` is the runtime's
/// invariant, not a limit invented here: `Mutator::new_class_object` is
/// the only thing that mints a class object and always gives it the shared
/// bootstrap `Object` placeholder -- §3.2's reified metaclass model stays
/// deliberately unbuilt -- so this refuses nothing a real world contains.
/// It is load-bearing because `class` is the one reference `Object`'s
/// iterative `Drop` cannot hand to its worklist (it is not an `Option`, and
/// the field's glue runs after `drop` returns): bounding the chain at one
/// link is what keeps a crafted image from cascading through it.
///
/// It also holds every closure frame to naming a closure object. A closure
/// frame's kind names an object-table id, and, like a tag-2 class
/// reference, whether that record was a closure is only knowable once every
/// record has been read. Everything that reaches a closure activation's
/// body or captured frames reads through that object.
fn check_class_references<'gc>(
    heap: &Heap<'gc>,
    objects: &[ObjectReference<'gc>],
    frames: &[FrameReference<'gc>],
    root_class: ClassReference<'gc>,
) -> Result<(), DecodeError> {
    for object in objects {
        let class = heap.class_of_object(*object);
        if !heap.is_class(class) {
            return Err(DecodeError::NotAClassReference);
        }
        if heap.is_class(*object) {
            if !is_prebuilt_class(heap, class, root_class) {
                return Err(DecodeError::ClassOfAClassIsARecord);
            }
            let superclass = heap.class_superclass(*object);
            let declaring_class = heap.class_declaring(*object);
            for class in superclass.iter().chain(declaring_class.iter()) {
                if !heap.is_class(*class) {
                    return Err(DecodeError::NotAClassReference);
                }
            }
        }
    }
    for frame in frames {
        let current_class = heap.frame_current_class(*frame);
        let kind = heap.frame_kind(*frame);
        if let FrameKind::Closure(closure) = &kind {
            if !heap.is_closure(*closure) {
                return Err(DecodeError::NotAClosureReference);
            }
        }
        let kind_class = match &kind {
            FrameKind::Initializer(class)
            | FrameKind::SuperclassClause(class)
            | FrameKind::SuperclassMessage(class) => Some(class),
            FrameKind::Method(_) | FrameKind::Closure(_) | FrameKind::TopLevel => None,
        };
        for class in current_class.iter().chain(kind_class) {
            if !heap.is_class(*class) {
                return Err(DecodeError::NotAClassReference);
            }
        }
    }
    Ok(())
}

/// Two-pass decoding: placeholder-allocate every object and every frame,
/// then fill each in place (so cycles and shared references round-trip
/// with identity preserved), then decode the roots.
///
/// Class objects need one more step between the two: their records are
/// filled in like any other, but the `MixinApplication` a class object
/// carries is derived rather than stored, and deriving it needs the
/// superclass's -- which may itself be a record that appears later.
/// `rebuild_applications` closes that gap once every record has been read.
/// It runs before the frames, whose `FrameKind::Method` resolves a selector
/// against `current_class`'s method dictionary.
///
/// Every pass is a flat loop over the two tables, and every reference
/// between records is an id into them, never an inline nested record -- so
/// no part of this decoder recurses on a depth the image controls. The one
/// depth-bounded structure in the format, `Literal::Array`, lives in the
/// code table (`decode_literal`'s `MAX_LITERAL_DEPTH`).
pub fn decode_graph_section<'gc>(
    reader: &mut Reader,
    context: &mut DecodeContext<'_, 'gc>,
    symbols: &symbols::Table,
) -> Result<DecodedGraph<'gc>, DecodeError> {
    let object_count = bounded_count(reader.read_u32()? as usize, MIN_OBJECT_RECORD_BYTES, reader)?;
    let frame_count = bounded_count(reader.read_u32()? as usize, MIN_FRAME_RECORD_BYTES, reader)?;
    let placeholder_class = context.mutator.builtin_class(BuiltinClass::Object);
    // A real application, standing in until `rebuild_applications` derives
    // each class object's own -- the same technique as the placeholder
    // object and `placeholder_frame`, and for the same reason: nothing may
    // observe a half-built record, and a `None` here would need an
    // `Option` on `ClassData` that no live class object could ever be.
    let placeholder_application = context
        .mutator
        .heap
        .class_application(placeholder_class)
        .ok_or(DecodeError::NotAClass)?;
    let objects: Vec<ObjectReference> = (0..object_count)
        .map(|_| {
            context
                .mutator
                .heap
                .allocate_object(placeholder_class, ObjectData::Instance(Vec::new()))
        })
        .collect();
    let frames: Vec<FrameReference> = (0..frame_count)
        .map(|_| context.mutator.heap.allocate_frame(placeholder_frame()))
        .collect();
    let tables = Tables {
        objects: &objects,
        frames: &frames,
    };
    let mut max_identity_hash = 0;
    let mut pending_classes: Vec<PendingClass> = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        let class = decode_class_tag(reader, context, &tables)?;
        let identity_hash = reader.read_u32()?;
        // Checked here, the one place identity hashes are read: `load`
        // hands the maximum straight to `advance_identity_hash_past`, which
        // must be able to allocate past it.
        if identity_hash == u32::MAX {
            return Err(DecodeError::IdentityHashExhausted);
        }
        max_identity_hash = max_identity_hash.max(identity_hash);
        let data = match reader.read_u8()? {
            OBJECT_INSTANCE => ObjectData::Instance(decode_values(reader, context, &tables)?),
            OBJECT_STRING => ObjectData::String(reader.read_str()?),
            OBJECT_SYMBOL => ObjectData::Symbol(decode_symbol(reader, symbols)?),
            OBJECT_ARRAY => ObjectData::Array(decode_values(reader, context, &tables)?),
            OBJECT_CLOSURE => {
                ObjectData::Closure(decode_closure_record(reader, context, &tables, symbols)?)
            }
            OBJECT_CLASS => {
                let (data, mixin) =
                    decode_class_record(reader, context, &tables, &placeholder_application)?;
                pending_classes.push(PendingClass {
                    object: index,
                    mixin,
                });
                ObjectData::Class(data)
            }
            tag => {
                return Err(DecodeError::InvalidTag {
                    what: "object body",
                    tag,
                })
            }
        };
        context.mutator.heap.replace_object(
            *object,
            Object {
                class,
                identity_hash,
                data,
            },
        );
    }
    rebuild_applications(context, &objects, pending_classes)?;
    for frame in &frames {
        let decoded = decode_frame(reader, context, &tables, symbols)?;
        context.mutator.heap.replace_frame(*frame, decoded);
    }
    check_class_references(
        &context.mutator.heap,
        &objects,
        &frames,
        *context.root_class,
    )?;
    let roots = decode_values(reader, context, &tables)?;
    Ok(DecodedGraph {
        roots,
        max_identity_hash,
        objects,
        frames,
    })
}

/// The inverse of `encode::encode_tag_named_hashes`: puts each recorded hash
/// back onto the object the bootstrap -- or `build_root_class` -- built, and
/// answers the largest one, which `load` advances the counter past together
/// with the graph's own maximum. A 0, a repeated object and `u32::MAX` are
/// refused, each for the reason its `DecodeError` gives.
pub fn decode_tag_named_hashes<'gc>(
    reader: &mut Reader,
    mutator: &mut Mutator<'gc>,
    root_class: ClassReference<'gc>,
) -> Result<u32, DecodeError> {
    let count = bounded_count(
        reader.read_u32()? as usize,
        MIN_TAG_NAMED_HASH_BYTES,
        reader,
    )?;
    let mut restored: Vec<ObjectReference> = Vec::new();
    let mut max = 0;
    for _ in 0..count {
        let target = match reader.read_u8()? {
            HASHED_BUILTIN_CLASS => mutator.builtin_class(read_builtin_class(reader)?),
            HASHED_ROOT_CLASS => root_class,
            HASHED_PLATFORM => match mutator.platform_value() {
                Value::Object(platform) => platform,
                _ => unreachable!("the platform singleton is always an object"),
            },
            tag => {
                return Err(DecodeError::InvalidTag {
                    what: "tag-named hash",
                    tag,
                })
            }
        };
        let hash = reader.read_u32()?;
        if hash == 0 {
            return Err(DecodeError::UnassignedTagNamedHash);
        }
        if hash == u32::MAX {
            return Err(DecodeError::IdentityHashExhausted);
        }
        if restored.iter().any(|seen| seen.same(target)) {
            return Err(DecodeError::DuplicateTagNamedHash);
        }
        restored.push(target);
        mutator.heap.set_identity_hash(target, hash);
        max = max.max(hash);
    }
    Ok(max)
}

/// The inverse of `encode::encode_class_record`: the mixin-table row this
/// class applies, then its superclass, its enclosing object and its
/// declaring class.
///
/// The mixin id is range-checked here, against the decoded mixin table,
/// before it is used to index anything. The three references resolve
/// to identities that already exist (every record was placeholder-allocated
/// before this pass started), so a forward reference and a cycle are both
/// ordinary; what cannot be settled yet is whether each really is a class
/// record, which `check_class_references` establishes afterwards.
fn decode_class_record<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    placeholder_application: &Rc<crate::mixin::MixinApplication>,
) -> Result<(crate::types::ClassData<'gc>, Rc<crate::mixin::Mixin>), DecodeError> {
    let id = reader.read_u32()?;
    let mixin = context
        .mixins
        .get(id as usize)
        .cloned()
        .ok_or(DecodeError::BadMixinId(id))?;
    let superclass = decode_optional_class(reader, context, tables)?;
    let enclosing = if reader.read_boolean()? {
        match decode_value(reader, context, tables)? {
            Value::Object(object) => Some(object),
            _ => return Err(DecodeError::EnclosingNotAnObject),
        }
    } else {
        None
    };
    let declaring_class = decode_optional_class(reader, context, tables)?;
    Ok((
        crate::types::ClassData {
            application: placeholder_application.clone(),
            enclosing,
            declaring_class,
            superclass,
            // A record only ever describes a user class: a builtin is a
            // class *tag*, resolved through the bootstrap, and never an
            // object-table entry.
            builtin: None,
            // Rebuilt by `Mutator::metaclass_of` on the first
            // class-side send, exactly as it is for a class object the
            // accessor just created.
            //
            // Not the *same* metaclass, though, in one case: a metaclass
            // that was itself in the graph -- as a class-side activation's
            // `current_class` -- decodes as a class record of its own and
            // is never linked back into the class it belongs to, so
            // `metaclass_of` mints a second one beside it. Nothing observes
            // the difference today: `metaclass_of` is reached only from
            // `Interpreter::send`'s dispatch, so no Newspeak value names a
            // metaclass and no two of them can be compared. Recording it
            // rather than claiming stability, because the claim would stop
            // being true the moment §3.2's reified model is built.
            metaclass: None,
        },
        mixin,
    ))
}

// ---------------------------------------------------------------------------
// Reconciling the decoded graph with the code that runs in it.
//
// `decode_graph_section` reads frames and closures in isolation: it has no
// selector-to-body mapping, so it can check a closure record's own two
// halves (`ClosureArityMismatch`) and nothing more. Once the decoded mixins
// carry their code, every frame's body can be resolved --
// and must be, because `locals::read_local` splits a frame at `arguments.len()`
// and indexes `temporaries` unchecked. A crafted frame that is smaller than the
// body addressing it is an out-of-bounds index reached from a
// version-matching image.
// ---------------------------------------------------------------------------

/// The compiled body that runs (or ran) in `frame`, or `None` when the
/// image gives it none: a `TopLevel` frame, or a class-carrying frame whose
/// kind names a body that class's mixin does not carry. Neither can be
/// entered -- nothing re-activates a decoded frame, and no closure may be
/// captured by one (`verify_closure` refuses that) -- so there is nothing to
/// reconcile.
fn frame_body<'gc>(heap: &Heap<'gc>, frame: FrameReference<'gc>) -> Option<Rc<CompiledCode>> {
    /// The mixin whose code a class-carrying frame kind names. `None` only
    /// if the decoded value is not a class object -- `decode_class_tag`
    /// never produces one, but returning `None` keeps a future change to it
    /// from turning an untrusted image into a panic here.
    fn frame_mixin<'gc>(
        heap: &Heap<'gc>,
        class: crate::types::ClassReference<'gc>,
    ) -> Option<Rc<crate::mixin::Mixin>> {
        heap.class_application(class)
            .map(|application| application.mixin.clone())
    }
    match &heap.frame_kind(frame) {
        FrameKind::Method(method) => Some(method.code.clone()),
        FrameKind::Initializer(class) => {
            frame_mixin(heap, *class).and_then(|mixin| mixin.initializer_code.clone())
        }
        // Each clause activation resolves against the field its own body
        // lives in. They share `Initializer`'s frame shape -- a class's code
        // running on an instance -- but not its body, so resolving them here
        // as initializers would reconcile a frame against a body that never
        // ran in it: a legitimate image refused, and a crafted one checked
        // against the wrong register file.
        FrameKind::SuperclassClause(class) => {
            frame_mixin(heap, *class).and_then(|mixin| mixin.superclass_clause_code.clone())
        }
        FrameKind::SuperclassMessage(class) => {
            frame_mixin(heap, *class).and_then(|mixin| mixin.superclass_message_code.clone())
        }
        FrameKind::Closure(closure) => heap.closure_code(*closure),
        FrameKind::TopLevel => None,
    }
}

/// Every decoded frame and every decoded closure against the code the
/// decoded mixins carry, closing the three gaps a per-record decoder
/// cannot see:
///
/// 0. A frame's `program_counter` must be an instruction index of the body
///    that runs in it. A frame's `program_counter` counts instructions (an
///    image carries only bytecode), but which body it counts them in is only
///    known once the decoded mixins carry their code.
/// 1. A frame's register file must match the body that runs in it -- the
///    same `arguments` count, and at least as many registers. (At least, not
///    exactly: a frame's register file is grown, never shrunk -- the
///    liveness spec's reified activations may be wider than the body
///    currently running in them -- so a wider frame is legitimate and only
///    a narrower one is a crafted image.)
/// 2. A closure's `captured_frame` must be an activation of the body its
///    own body is lexically nested in. Nothing else in the format ties the
///    two together, and `verify_code_table`'s outer-local bound -- which
///    walks the *static* `MakeClosure` nesting -- is only a bound on the
///    real `frame_at_depth` walk because of this.
pub fn verify_graph<'gc>(
    graph: &DecodedGraph<'gc>,
    table: &DecodedCodeTable,
    heap: &Heap<'gc>,
) -> Result<(), DecodeError> {
    // Recomputed rather than threaded through from `verify_code_table`:
    // one more linear pass over the code table at load time, in exchange
    // for a function that is safe to call on its own.
    let parents = lexical_parents(table)?;
    let index_of = code_index(table);
    for frame in &graph.frames {
        let Some(code) = frame_body(heap, *frame) else {
            continue;
        };
        let (program_counter, argument_count, temporary_count) = heap.frame_register_shape(*frame);
        // The VM stores the index of the instruction it is
        // executing, and a body with no instructions is already refused
        // (`UnterminatedBody`), so every genuine activation -- including one
        // that has not run a single instruction yet -- satisfies this.
        if program_counter >= code.instructions.len() {
            return Err(DecodeError::ProgramCounterOutOfRange {
                program_counter,
                instructions: code.instructions.len(),
            });
        }
        if argument_count != code.argument_count as usize
            || argument_count + temporary_count < code.register_count as usize
        {
            return Err(DecodeError::FrameRegisterFile {
                arguments: argument_count,
                locals: temporary_count,
                argument_count: code.argument_count,
                register_count: code.register_count,
            });
        }
    }
    // Every closure the graph holds is an object record -- a closure frame
    // names one of these (`check_class_references` refused any that does
    // not) -- so this one loop also covers the closure of every closure
    // activation.
    for object in &graph.objects {
        if let Some(closure) = heap.closure_data(*object) {
            verify_closure(&closure, table, &parents, &index_of, heap)?;
        }
    }
    Ok(())
}

fn verify_closure<'gc>(
    closure: &ClosureData<'gc>,
    table: &DecodedCodeTable,
    parents: &[Option<usize>],
    index_of: &HashMap<usize, usize>,
    heap: &Heap<'gc>,
) -> Result<(), DecodeError> {
    let code = &closure.code;
    if code.kind != CodeKind::Block {
        return Err(DecodeError::WrongCodeKind { expected: "block" });
    }
    let id = *index_of
        .get(&(Rc::as_ptr(code) as usize))
        .ok_or(DecodeError::CodeNotInTable)?;
    // A body no `MakeClosure` creates has no lexical parent to check
    // against, and `enclosing_body` would stop at the body itself -- a
    // bound the real chain need not respect. Refusing keeps the two walks
    // in step.
    let parent = parents[id].ok_or(DecodeError::CapturedFrameMismatch)?;
    let running =
        frame_body(heap, closure.captured_frame).ok_or(DecodeError::CapturedFrameMismatch)?;
    if !Rc::ptr_eq(&running, &table.codes[parent]) {
        return Err(DecodeError::CapturedFrameMismatch);
    }
    Ok(())
}
