//! Canonical decoding, the inverse of `encode.rs`. A malformed byte stream
//! is a `DecodeError`, never a panic.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use ns_syntax::Span;

use crate::bytecode::{
    CallSite, CodeBuilder, CodeKind, CompiledCode, CreatedBlock, Emitted, Instruction,
    IntegerOperation, Literal,
};
use crate::heap::{Heap, Word};
use crate::interpreter::instantiate;
use crate::mixin::DispatchId;
use crate::types::{
    fits_small_integer, BuiltinClass, ClassReference, ClosureData, Frame, FrameKind,
    FrameReference, Object, ObjectData, ObjectReference, SourceOrigin, Value,
};
use crate::Symbol;
use crate::{ArbitraryInteger, Mutator};

use super::modules::ModuleClasses;
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
    /// A start image names its application by being the graph's one root; any
    /// other count leaves the host nothing, or nothing certain, to start.
    StartedImageRootCount {
        count: usize,
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
    /// afterwards, twice over. The depth count exempts a control-flow
    /// step's block activated for a `SendInlinable` when the
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
    /// A class record and a metaclass record do not pair up one to one: a
    /// class record's header -- its class -- names something other than a
    /// metaclass record whose reference back is that class record; a
    /// metaclass record's reference back names something other than a class
    /// record, or a class record whose header names another metaclass; or a
    /// metaclass record's own header is not `Metaclass`. A live class has exactly one
    /// metaclass and that metaclass exactly one class (§3.2, "unique"), so
    /// no real world encodes anything else, and a class that answered one
    /// metaclass while another claimed it would give metaclass identity two
    /// answers.
    MetaclassPairing,
    /// A class record applies some row's class side. A metaclass is written
    /// as a record of its own (`OBJECT_METACLASS`) whose application follows
    /// from its class, so a class record applying a class side describes a
    /// second, unpaired metaclass no live world has.
    ClassRecordAppliesAClassSide,
    /// An activation record (`OBJECT_ACTIVATION`) whose header is not the
    /// builtin `Activation`, or a record of another kind whose header is.
    /// Only `Mutator::new_activation_object` makes an activation object, and
    /// it gives every one that class and nothing else that class, so a
    /// mismatch describes an object no live world has -- an activation
    /// answering some other class's methods, or an ordinary object answering
    /// `Activation`'s, which read other frames and unwind the stack.
    ActivationHeader,
    /// A record under the builtin `Future` header that is not an unresolved
    /// future's: an instance record of exactly two fields, a closure and the
    /// state 0. The encoder writes a resolved future as its value and a
    /// resolving one as unresolved (`docs/design/image-format.md`, "Futures
    /// in a graph"), so any other record describes a future no encoder
    /// wrote -- one `Mutator::future_state` reads as malformed, or a resolved one
    /// standing apart from the value every other reference was written as.
    FutureRecord,
    /// An object record that is not a class object is an instance of a class
    /// whose chain reaches `Behavior` -- `Class`, `Metaclass` or a
    /// metaclass. A class object is made only by evaluating a declaration
    /// (§3.2; `InstantiationRefusal::ClassObject`), so no source can make
    /// such an instance, and one decoded would be an object every method of
    /// those classes assumes is a class.
    InstanceOfAClassClass,
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
    /// The mixin table's rows are not `mixin::preorder` of each tree's first
    /// row, tree after tree: a row is named as a child by two rows, a row is
    /// its own ancestor or reaches back into an earlier tree, or the children
    /// are named out of the order the walk visits them in.
    /// Row position is the whole correspondence between a row and the
    /// declaration it describes, so a table that does not reproduce that
    /// walk is refused outright rather than paired up wrongly.
    MixinTableNotATree {
        row: u32,
    },
    /// The mixin table holds a different number of declaration trees than
    /// the image declares modules. Each tree's first row is a module's root,
    /// so a surplus tree -- a row nothing reaches -- would be a module no
    /// part of the image names, and a missing one a module without a root.
    ModuleCountMismatch {
        declared: u32,
        trees: u32,
    },
    /// One mixin-table row declares a member twice: a method, a slot's
    /// accessor or a nested class under a name another of them already has.
    /// Its rebuilt method table would silently keep only one of the two, and
    /// the resolver refuses such a declaration from source.
    DuplicateSelector(String),
    /// A mixin-table row declares a lazy slot (§6.3.3) but no method that
    /// can be its getter: a unary method of the slot's name with the slot's
    /// accessibility. The getter is one of the row's methods, not something
    /// the decoder derives, so without it no send could ever read the slot.
    LazySlotWithoutGetter(String),
    /// A selector or name reference (a send's selector, a method-dictionary
    /// key, a closure's parameter name, a `Symbol` object's text, ...) that
    /// indexes past the end of the image's symbol table. Every such
    /// reference is a `u32` id checked against the table at the point of
    /// use, never trusted to have been validated elsewhere.
    BadSymbolId(u32),
    /// A `MakeClosure`'s parameter-name list and the block body it names
    /// disagree about how many arguments the closure takes. The two are
    /// separate fields on the wire, so only a decode-time reconciliation can
    /// catch a crafted mismatch. Nothing at run time reads the name list --
    /// `Heap::closure_parameter_count` answers from the body, so the arity a
    /// call is checked against and the arity the register file is sized from
    /// are one number -- but `bytecode::disassemble` prints the names, and
    /// `inspect` reads them off the same field. A mismatch left standing
    /// would make a decoded image describe itself wrongly, with nothing
    /// reporting it, so it is refused instead. A closure *record* no longer
    /// has this half at all: it carries copied values, not parameter names
    /// (`CopiedCountMismatch` is what reconciles a record).
    ClosureArityMismatch {
        parameters: usize,
        argument_count: u16,
    },
    /// A `MakeClosure`'s copied-register list, or a closure record's copied
    /// values, does not number what the block body it names declares. The
    /// two are separate wire fields, and the block's activation is sized and
    /// filled from `copied_count` (`Heap::fill_copied_values`) while the
    /// list is what supplies the values: a shorter list leaves a register
    /// the body reads as a plain local holding nil, and a longer one hands
    /// values to registers that are somebody else's temporaries.
    CopiedCountMismatch {
        copied: usize,
        copied_count: u16,
    },
    /// A code record's parameters and copied values together do not fit its
    /// register file. `push_step` sizes an activation as `register_count -
    /// argument_count`, so this would leave `fill_copied_values` writing
    /// past the registers the body has.
    CopiedCountOutOfRange {
        argument_count: u16,
        copied_count: u16,
        register_count: u16,
    },
    /// A class record names a `<:` clause its applying row does not declare:
    /// `index` is past the row's clauses, or -- in a module's root section --
    /// the root's levels are not listed in clause order, one per clause at
    /// most. A level's clause is where its selector and compiled message come
    /// from, so there is nothing a level with no clause could initialize by.
    BadMixinApplicationClause {
        row: u32,
        index: u32,
    },
    /// A mixin-table row's `<:` clause passes factory arguments the image has
    /// no compiled body for. `instantiate::clause_message_body` panics on the
    /// invariant when the clause carries none -- the same refusal, for the
    /// same reason, as `MissingSuperclassMessage`.
    MissingMixinApplicationMessage,
    /// A mixin-table row's `<:` clause passes no factory arguments, yet names
    /// a compiled message body. The compiler fills a clause's body only when
    /// its message has arguments, and the walk never runs one for a clause
    /// without them (`instantiate::applied_clause_message_body`), so such a
    /// body is code the row says nothing runs -- while a frame of kind
    /// `MixinApplicationMessage` could still name it. Refused so a row and the
    /// frames running its clauses cannot disagree about what a clause runs.
    UnexpectedMixinApplicationMessage,
    /// A mixin-table row declares `<:` clauses but no superclass clause. The
    /// grammar admits `<:` only after a superclass clause, and it is the
    /// superclass clause's body that evaluates every clause's receiver
    /// (`interpreter::nested::superclass_clause_expression`), so such a row
    /// describes a declaration no source has and whose levels nothing builds.
    MixinApplicationWithoutSuperclass,
    /// A module's root section names no superclass for its root
    /// class. The root rule builds every root over `Object` or `Value`
    /// (`resolver::root_clause_defect`), so the encoder never
    /// writes one without, and a root with none would be a class outside
    /// §3.2's hierarchy, below neither `Object` nor `Top`.
    RootWithoutSuperclass,
    /// A `CLASS_MODULE` class tag or `HASHED_MODULE_CLASS` entry names a
    /// module past the ones the module section declares.
    BadModuleReference {
        module: u32,
    },
    /// A `CLASS_MODULE_LEVEL` class tag or `HASHED_MODULE_LEVEL` entry names
    /// a module past the ones the module section declares, or a position
    /// past the levels that module's root section rebuilt.
    BadModuleLevel {
        module: u32,
        position: u32,
    },
    /// A module has no tree in the image's mixin table, so it describes no
    /// root class -- nothing for the graph's `CLASS_MODULE` tag to name and
    /// no declaration to instantiate.
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
    /// through: the VM fetches the instruction at `program_counter` after
    /// every fall-through, so either runs off the end of the body.
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
    /// A graph section holds two object records for the same symbol text. A
    /// canonical heap never encodes two: merging them would mean rewriting
    /// every reference to the second, so the image is refused instead
    /// (`docs/design/image-format.md`, "Verification").
    DuplicateSymbol(String),
    /// An `OBJECT_SYMBOL` record's class tag names something other than the
    /// builtin `Symbol` class. `decode_graph_section` enters every symbol
    /// record into the heap's canonical table as it is decoded, before
    /// anything checks the record's class is even a class the language could
    /// have produced a symbol from -- every later literal of that text would
    /// answer the mis-classed object, so the image is refused instead of
    /// trusted (`docs/design/image-format.md`, "Verification").
    SymbolOfWrongClass(String),
    /// Bytes remain after the last section. Refused rather than ignored:
    /// silently accepting a tail means a padded image loads as something its
    /// author did not write.
    TrailingBytes {
        count: usize,
    },
    /// The platform path section lists more paths than
    /// `platform_paths::MAX_PLATFORM_PATHS`. Each path costs the loader
    /// sends, so the count is bounded before any is made.
    TooManyPlatformPaths {
        count: usize,
    },
    /// A platform path of no members -- the empty path is the platform,
    /// which `VALUE_PLATFORM` names -- or of more than
    /// `platform_paths::MAX_PLATFORM_PATH_LENGTH`.
    PlatformPathLength {
        length: usize,
    },
    /// A platform path member that is not a unary selector. A path is a
    /// chain of unary sends, so no other selector can be one of its steps.
    PlatformPathMemberNotUnary(String),
    /// A platform path reference whose index is past the image's platform
    /// path section.
    BadPlatformPath(u32),
    /// A class reference names a platform path whose object is not a class.
    PlatformPathNotAClass,
    /// An `OBJECT_INSTANCE` record of a class the loading platform names by
    /// path carries a field count other than that class's layout. The layout
    /// comes from the loading binary, not from the image
    /// (`docs/design/core-library.md`, "Platform paths in the image"), so a
    /// mismatch would leave the class's code reading fields the record does
    /// not have, or not reading ones it does.
    PlatformInstanceFieldCount {
        expected: usize,
        found: usize,
    },
    /// A `LargeInteger`, as a value or a literal, in an encoding the encoder
    /// never writes: one that fits `i64` (which travels as an `i64`), a zero
    /// top digit, or a negative zero. Accepting one would give an integer a
    /// second encoding, and a decoder that trusted the tag rather than the
    /// range would load a second representation -- a `LargeInteger` object
    /// equal to a `SmallInteger`, which every arithmetic fast path would
    /// treat as a different value.
    NonCanonicalInteger,
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
            DecodeError::StartedImageRootCount { count } => write!(
                f,
                "a started image has exactly one root, the application, but this one has {count}"
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
            DecodeError::MetaclassPairing => write!(
                f,
                "a class in the image and the metaclass it names do not belong to each other"
            ),
            DecodeError::ClassRecordAppliesAClassSide => write!(
                f,
                "a class object in the image applies a class side, which only its metaclass may"
            ),
            DecodeError::ActivationHeader => write!(
                f,
                "an object in the image is an Activation without an activation's record, \
                 or an activation's record under another class"
            ),
            DecodeError::FutureRecord => write!(
                f,
                "an object in the image is a Future without an unresolved future's record"
            ),
            DecodeError::InstanceOfAClassClass => write!(
                f,
                "an object in the image is an instance of Class, Metaclass or a metaclass, \
                 which only a class object may be"
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
            DecodeError::ModuleCountMismatch { declared, trees } => write!(
                f,
                "the image declares {declared} module(s) but its mixin table holds {trees} tree(s)"
            ),
            DecodeError::DuplicateSelector(selector) => write!(
                f,
                "a mixin-table row declares member `{selector}` more than once"
            ),
            DecodeError::LazySlotWithoutGetter(slot) => write!(
                f,
                "a mixin-table row declares the lazy slot `{slot}` without its getter method"
            ),
            DecodeError::ClosureArityMismatch { parameters, argument_count } => write!(
                f,
                "a `MakeClosure` names {parameters} parameters but the block body it creates takes {argument_count}"
            ),
            DecodeError::CopiedCountMismatch { copied, copied_count } => write!(
                f,
                "the image supplies {copied} copied values for a block that is handed {copied_count}"
            ),
            DecodeError::CopiedCountOutOfRange { argument_count, copied_count, register_count } => write!(
                f,
                "a body in the image takes {argument_count} arguments and {copied_count} copied \
                 values, which do not fit its {register_count} registers"
            ),
            DecodeError::BadMixinApplicationClause { row, index } => write!(
                f,
                "the image names mixin application clause {index} of mixin row {row}, which the \
                 row does not declare in that place"
            ),
            DecodeError::MissingMixinApplicationMessage => write!(
                f,
                "the image's mixin application clause passes factory arguments it has no compiled code for"
            ),
            DecodeError::UnexpectedMixinApplicationMessage => write!(
                f,
                "the image's mixin application clause passes no factory arguments but carries compiled code for them"
            ),
            DecodeError::MixinApplicationWithoutSuperclass => write!(
                f,
                "a mixin-table row applies mixins but declares no superclass clause to apply them to"
            ),
            DecodeError::RootWithoutSuperclass => write!(
                f,
                "the image's root class inherits from nothing, but a module's root class must inherit Object or Value"
            ),
            DecodeError::BadModuleReference { module } => write!(
                f,
                "the image names module {module}, which its module section does not declare"
            ),
            DecodeError::BadModuleLevel { module, position } => write!(
                f,
                "the image names level {position} of module {module}, which its module section \
                 does not rebuild"
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
            DecodeError::DuplicateSymbol(text) => {
                write!(f, "symbol #{text} is recorded twice")
            }
            DecodeError::SymbolOfWrongClass(text) => {
                write!(f, "symbol #{text} is recorded with a class other than Symbol")
            }
            DecodeError::TrailingBytes { count } => {
                write!(f, "{count} trailing byte(s) after the last section")
            }
            DecodeError::TooManyPlatformPaths { count } => write!(
                f,
                "the image lists {count} platform paths, more than the {} a loader follows",
                super::platform_paths::MAX_PLATFORM_PATHS
            ),
            DecodeError::PlatformPathLength { length } => write!(
                f,
                "a platform path in the image has {length} members, but a path has between 1 and {} members",
                super::platform_paths::MAX_PLATFORM_PATH_LENGTH
            ),
            DecodeError::PlatformPathMemberNotUnary(member) => write!(
                f,
                "platform path member `{member}` is not a unary selector"
            ),
            DecodeError::BadPlatformPath(index) => {
                write!(f, "platform path {index} out of range")
            }
            DecodeError::PlatformPathNotAClass => write!(
                f,
                "an image field that must name a class names a platform path whose object is not a class"
            ),
            DecodeError::PlatformInstanceFieldCount { expected, found } => write!(
                f,
                "an instance of a class the platform names carries {found} field(s), but the loading \
                 binary lays that class out with {expected}"
            ),
            DecodeError::NonCanonicalInteger => {
                write!(f, "an integer is not in its one canonical encoding")
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

    pub fn read_u64(&mut self) -> Result<u64, DecodeError> {
        let bytes: [u8; 8] = self
            .read_raw(8)?
            .try_into()
            .expect("read_raw(8) returns 8 bytes");
        Ok(u64::from_le_bytes(bytes))
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

    /// A String record's text, validated as UTF-8 where it lies and
    /// converted straight from the buffer: no intermediate `Vec` or
    /// `String`, so a record costs at most the 4 heap bytes per wire byte
    /// of a wide string (`docs/design/memory.md`, the decode
    /// amplification).
    pub fn read_code_points(&mut self) -> Result<crate::CodePoints, DecodeError> {
        let length = self.read_u32()? as usize;
        let bytes = self.read_raw(length)?;
        let text = std::str::from_utf8(bytes).map_err(|_| DecodeError::InvalidUtf8)?;
        Ok(crate::CodePoints::from(text))
    }
}

/// What the header says about the rest of the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Whether every module in the module section carries its source text --
    /// the debug variant. The source is never executed and never re-parsed;
    /// it is carried so a loaded world can be shown, and eventually exported,
    /// as `.ns`.
    pub has_source: bool,
    /// The graph's one root is an application to start (`FLAG_START`).
    pub start: bool,
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
        start: flags & super::encode::FLAG_START != 0,
    })
}

/// The inverse of `encode::encode_source_origin`: the provenance a loaded
/// world reports for a module.
pub fn decode_source_origin(reader: &mut Reader) -> Result<SourceOrigin, DecodeError> {
    match reader.read_u8()? {
        ORIGIN_FILE => Ok(SourceOrigin::File(PathBuf::from(reader.read_str()?))),
        ORIGIN_EMBEDDED => Ok(SourceOrigin::Embedded(reader.read_str()?)),
        tag => Err(DecodeError::InvalidTag {
            what: "source origin",
            tag,
        }),
    }
}

pub fn decode_scalar_value<'gc>(
    mutator: &mut Mutator<'gc>,
    reader: &mut Reader,
) -> Result<Value<'gc>, DecodeError> {
    match reader.read_u8()? {
        0 => Ok(Value::nil()),
        1 => Ok(Value::boolean(false)),
        2 => Ok(Value::boolean(true)),
        3 => Ok(integer_value(mutator, reader.read_i64()?)),
        4 => Ok(mutator.new_float(reader.read_f64()?)),
        tag => Err(DecodeError::InvalidTag { what: "value", tag }),
    }
}

/// An integer the wire carries as an `i64`, as the one representation it
/// has: a `SmallInteger` in 61 bits, without allocating, which is nearly
/// every one; a `LargeInteger` beyond them.
fn integer_value<'gc>(mutator: &mut Mutator<'gc>, number: i64) -> Value<'gc> {
    if fits_small_integer(number) {
        Value::small_integer(number)
    } else {
        mutator.new_integer(ArbitraryInteger::from_i64(number))
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
        9 => BuiltinClass::Array,
        11 => BuiltinClass::UndefinedObject,
        12 => BuiltinClass::Block,
        14 => BuiltinClass::VmMirror,
        15 => BuiltinClass::Top,
        16 => BuiltinClass::Value,
        17 => BuiltinClass::Number,
        20 => BuiltinClass::Behavior,
        21 => BuiltinClass::Class,
        22 => BuiltinClass::Metaclass,
        23 => BuiltinClass::Activation,
        24 => BuiltinClass::Exception,
        25 => BuiltinClass::Error,
        26 => BuiltinClass::MessageNotUnderstood,
        27 => BuiltinClass::Message,
        28 => BuiltinClass::Halt,
        30 => BuiltinClass::Future,
        31 => BuiltinClass::LargeInteger,
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

/// A builtin class referenced in the graph: a class reference recorded as a
/// value (`CLASS_BUILTIN`), which from Task 4 on can hold `Metaclass` -- `3
/// class class class` reaches it. Four tags are refused here, because no
/// source module can produce them and an artifact that carried one would
/// decode into a program the language does not admit. The VM mirror's class:
/// there is exactly one mirror, and an artifact that could name its class
/// could instantiate or reach a second one, and so claim every primitive.
/// `Top`: §3.2 lets only `Object` have it as a superclass, so a class named
/// over `Top` would sit outside `Object`'s chain and inherit none of its
/// methods. `Activation`: a running activation as an object reads other
/// frames and unwinds stacks, a capability Newspeak hands out only through
/// mirrors, so no artifact may name its class either
/// (`docs/design/core-library.md`, "Exceptions") -- except as the
/// header of an activation record, which `decode_object_header` reads.
/// `Future`: §6.3.2's pull future is kernel-private as `Activation` is, and
/// no program names its class -- except as the header of an unresolved
/// future's record, which `decode_object_header` reads too. `Object`
/// declares an accessor for none of the four, for the same reasons, which
/// is what keeps source and artifact agreed on the names a program has.
/// Only the closing hash table names them, by tag, as it names every
/// builtin.
pub fn read_referenced_builtin_class(reader: &mut Reader) -> Result<BuiltinClass, DecodeError> {
    referenced_builtin_class(read_builtin_class(reader)?)
}

/// `read_referenced_builtin_class`'s refusals, of a builtin already read --
/// what `decode_object_header` applies to every header but an activation
/// record's and an unresolved future's.
fn referenced_builtin_class(kind: BuiltinClass) -> Result<BuiltinClass, DecodeError> {
    match kind {
        BuiltinClass::VmMirror => Err(DecodeError::InvalidTag {
            what: "builtin class a program may name",
            tag: 14,
        }),
        BuiltinClass::Top => Err(DecodeError::InvalidTag {
            what: "builtin class a program may name",
            tag: 15,
        }),
        BuiltinClass::Activation => Err(DecodeError::InvalidTag {
            what: "builtin class a program may name",
            tag: 23,
        }),
        BuiltinClass::Future => Err(DecodeError::InvalidTag {
            what: "builtin class a program may name",
            tag: 30,
        }),
        kind => Ok(kind),
    }
}

/// A builtin class a *program* names as a superclass or as the mixin of a
/// `<:` clause. `read_referenced_builtin_class`'s two refusals, and three
/// more: `Behavior`, `Class` and `Metaclass`, which `Object` declares no
/// accessor for, so no source module can name them either; a class over
/// one of them could never be instantiated (`InstantiationRefusal::ClassObject`).
pub fn read_program_builtin_class(reader: &mut Reader) -> Result<BuiltinClass, DecodeError> {
    let kind = read_referenced_builtin_class(reader)?;
    match kind {
        BuiltinClass::Behavior | BuiltinClass::Class | BuiltinClass::Metaclass => {
            Err(DecodeError::InvalidTag {
                what: "builtin class a program may name",
                tag: crate::image::encode::builtin_class_tag(kind),
            })
        }
        kind => Ok(kind),
    }
}

/// The inverse of `encode::integer_operation_tag`. An unrecognized byte is
/// refused rather than defaulted to an operation -- an image is untrusted
/// input, and a stale or corrupt byte must not decode into a different,
/// perfectly valid `IntegerBinary`.
pub fn integer_operation_from_tag(tag: u8) -> Option<IntegerOperation> {
    Some(match tag {
        0 => IntegerOperation::Add,
        1 => IntegerOperation::Subtract,
        2 => IntegerOperation::Multiply,
        3 => IntegerOperation::FloorDivide,
        4 => IntegerOperation::Less,
        5 => IntegerOperation::Greater,
        6 => IntegerOperation::LessOrEqual,
        7 => IntegerOperation::GreaterOrEqual,
        8 => IntegerOperation::Equal,
        _ => return None,
    })
}

fn read_integer_operation(reader: &mut Reader) -> Result<IntegerOperation, DecodeError> {
    let tag = reader.read_u8()?;
    integer_operation_from_tag(tag).ok_or(DecodeError::InvalidTag {
        what: "integer operation",
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
/// 128, private to that crate and so restated here. Every block literal is one `parse_primary`, one level of
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
    for block in code.body().created_blocks() {
        // `decode_instruction` resolved the id against the bodies already
        // read, and every one of them was measured.
        let below = nesting
            .get(&(Rc::as_ptr(block.code) as usize))
            .ok_or(DecodeError::CodeNotInTable)?;
        depth = depth.max(below + 1);
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
        let mut methods: Vec<&Rc<crate::mixin::MixinMethod>> = mixin.compiled_methods().collect();
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
        if !mixin.mixin_applications.is_empty() && mixin.superclass.is_none() {
            return Err(DecodeError::MixinApplicationWithoutSuperclass);
        }
        for (index, clause) in mixin.mixin_applications.iter().enumerate() {
            match &clause.message_code {
                Some(_) if !clause.has_arguments => {
                    return Err(DecodeError::UnexpectedMixinApplicationMessage)
                }
                // §6.2 evaluates a `<:` clause's message in the applying
                // declaration's instance initializer, as it does the superclass
                // clause's, so its body takes this mixin's factory arguments --
                // which is the precondition `instantiate`'s arity guard only
                // re-checks at run time, as a Newspeak error rather than a panic.
                Some(code) => check_code_arity(
                    code,
                    CodeKind::SuperclassMessage,
                    format!("mixin application clause {} message", index + 1),
                    mixin.factory.map_or(0, |factory| factory.argument_count),
                )?,
                None if clause.has_arguments => {
                    return Err(DecodeError::MissingMixinApplicationMessage)
                }
                None => {}
            }
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
// `code.slot(program_counter)` directly, and
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
        for CreatedBlock {
            code: block,
            parameters,
            copied,
        } in code.body().created_blocks()
        {
            let id = *index_of
                .get(&(Rc::as_ptr(block) as usize))
                .ok_or(DecodeError::CodeNotInTable)?;
            if block.kind != CodeKind::Block {
                return Err(DecodeError::WrongCodeKind { expected: "block" });
            }
            // The instruction's parameter names against the body's arity:
            // nothing runs on the names, but `disassemble` and `inspect`
            // print them, so a mismatch would be an image describing itself
            // wrongly rather than an error anyone sees.
            if parameters.len() != block.argument_count as usize {
                return Err(DecodeError::ClosureArityMismatch {
                    parameters: parameters.len(),
                    argument_count: block.argument_count,
                });
            }
            // The same reconciliation for the other half of a block's
            // register layout: the instruction supplies the values, the
            // body declares how many registers take them.
            if copied.len() != block.copied_count as usize {
                return Err(DecodeError::CopiedCountMismatch {
                    copied: copied.len(),
                    copied_count: block.copied_count,
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

/// Assigns every family's call-site numbers once every body in the table is
/// decoded: a block's numbers depend on where its `MakeClosure` sits in its
/// root's emission order, which only the flat table read back in full can
/// answer (`docs/design/dispatch.md`, "Decisions"). A root -- a body no
/// `MakeClosure` creates -- is found the same way `verify_code_table` finds
/// one, by reusing `lexical_parents`' scan, so this can run on its own
/// wherever `decode_code_table` succeeded, before `verify_code_table` or
/// after it.
///
/// A root's own table slot is its only reference at this point: nothing
/// creates a root via `MakeClosure` (`lexical_parents` already proved that),
/// and nothing has cloned anything out of the table yet -- `mixins::decode`,
/// the first thing that does, has not run. Mutating it in place through
/// `Rc::get_mut` therefore never allocates. A block's slot, by contrast, is
/// still shared with its parent's `MakeClosure` field, so numbering it
/// through that field (`Rc::make_mut`) clones the block away from the
/// table's own, still-unnumbered copy; `writeback` re-points the table at
/// the numbered clone afterwards, so `Rc::as_ptr` identity -- what
/// `code_index`, `lexical_parents` and `verify_closure` all key on -- still
/// agrees with what a decoded closure or frame points to later.
///
/// **Precondition, unenforced by the type system**: `table` must not have
/// been handed to anything that clones a code out of it (`mixins::decode`,
/// today's only such caller) before this runs. Both production callers
/// (`image::load`, `image::inspect::inspect`) call this immediately after
/// `decode_code_table`, before `mixins::decode`, which is what makes the
/// `Rc::get_mut` below never fail; nothing here checks it, since a `Cell`
/// or a runtime check would cost every call for a precondition its only two
/// callers already satisfy by construction.
pub fn number_call_site_families(table: &mut DecodedCodeTable) -> Result<(), DecodeError> {
    let parents = lexical_parents(table)?;
    let index_of = code_index(table);
    let mut writeback: Vec<(usize, Rc<CompiledCode>)> = Vec::new();
    for index in parents
        .iter()
        .enumerate()
        .filter(|(_, parent)| parent.is_none())
        .map(|(index, _)| index)
    {
        let root = Rc::get_mut(&mut table.codes[index]).expect(
            "internal: a root's own table slot is its only reference before the mixin table clones it",
        );
        let mut next = 0;
        number_call_sites(root, &mut next, &index_of, &mut writeback)?;
    }
    for (index, code) in writeback {
        table.codes[index] = code;
    }
    Ok(())
}

/// Numbers one code family as `compiler::lower` numbers it: one counter
/// threaded through a body's own sends and every block nested in it, in
/// emission order, so no two sends under one root ever share a number. The
/// number is a property of the code, never read from the artifact, so a
/// stale body cannot bring a number of its own. A send already `UNCACHED` --
/// the whole body was compiled by the reference lowering -- is left alone:
/// every send in such a body is `UNCACHED` (`encode::compiled_uncached`), so
/// there is nothing to number and `next` does not advance for it.
///
/// Recursion depth is the block-nesting depth, which `decode_code_table`
/// already bounded (`MAX_BLOCK_NESTING`) before this ever runs, so a crafted
/// artifact cannot use this walk to recurse further than that.
fn number_call_sites(
    code: &mut CompiledCode,
    next: &mut u32,
    index_of: &HashMap<usize, usize>,
    writeback: &mut Vec<(usize, Rc<CompiledCode>)>,
) -> Result<(), DecodeError> {
    // `for_each_emitted_mut` visits every send's call site and every block
    // in emission order, whichever form each was encoded in -- a wide one's
    // through `call_site_mut`, the same variant list `call_site` reads for
    // `family_size_of` -- so no call-site-carrying instruction can go
    // unnumbered here.
    code.body_mut().for_each_emitted_mut(|emitted| {
        match emitted {
            Emitted::Block(block) => {
                // Captured before `Rc::make_mut` can move the block to a
                // fresh allocation: `index_of` was built from the table's
                // original pointers, and `lexical_parents` already proved
                // this one names exactly one table entry.
                let block_index = *index_of
                    .get(&(Rc::as_ptr(block) as usize))
                    .ok_or(DecodeError::CodeNotInTable)?;
                number_call_sites(Rc::make_mut(block), next, index_of, writeback)?;
                writeback.push((block_index, block.clone()));
            }
            Emitted::CallSite(call_site) => {
                if *call_site != CallSite::UNCACHED {
                    *call_site = CallSite { number: *next };
                    *next += 1;
                }
            }
        }
        Ok(())
    })?;
    code.family_size = code.body().family_size();
    Ok(())
}

/// Every decoded body checked against what the VM will do with it:
/// its arity fits its register file, it cannot run past its last
/// instruction, every jump target is one of its own instructions, every
/// register operand is inside its own register file, and every outer local
/// is inside the register file of the body it addresses.
///
/// Each body is decoded here once, and both checks that read every
/// instruction read that one view; everything else at load reads the
/// encoding directly (`EncodedBody::created_blocks`,
/// `EncodedBody::family_size`).
pub fn verify_code_table(table: &DecodedCodeTable) -> Result<(), DecodeError> {
    let parents = lexical_parents(table)?;
    for (index, code) in table.codes.iter().enumerate() {
        let instructions = code.instructions();
        verify_body(code, &instructions)?;
        verify_outer_locals(table, &parents, index, &instructions)?;
    }
    Ok(())
}

fn verify_body(code: &CompiledCode, instructions: &[Instruction]) -> Result<(), DecodeError> {
    // Every activation is pushed with `register_count - argument_count`
    // temporaries, a `u16` subtraction.
    if code.argument_count > code.register_count {
        return Err(DecodeError::ArgumentsExceedRegisters {
            argument_count: code.argument_count,
            register_count: code.register_count,
        });
    }
    // The VM fetches the instruction at `program_counter` at the top of the
    // loop and again after every fall-through, so an empty body -- or one
    // whose last instruction falls through -- indexes past its end.
    match instructions.last() {
        Some(
            Instruction::Return { .. }
            | Instruction::ImplicitReturn { .. }
            | Instruction::NonLocalReturn { .. }
            | Instruction::Jump { .. },
        ) => {}
        _ => return Err(DecodeError::UnterminatedBody),
    }
    for instruction in instructions {
        for register in own_registers(instruction) {
            if register >= code.register_count {
                return Err(DecodeError::RegisterOutOfRange {
                    register,
                    register_count: code.register_count,
                });
            }
        }
        if let Some(target) = jump_target(instruction) {
            if target as usize >= instructions.len() {
                return Err(DecodeError::JumpTargetOutOfRange {
                    target,
                    instructions: instructions.len(),
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
        | Instruction::LoadEnclosingObject { destination, .. } => vec![*destination],
        // The copied registers are read out of the *executing* frame, like
        // a send's arguments, so they are bounded here and not by
        // `verify_outer_locals`.
        Instruction::MakeClosure {
            destination,
            copied,
            ..
        } => {
            let mut registers = copied.clone();
            registers.push(*destination);
            registers
        }
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
        // The decoder never produces one -- it has no wire tag -- but the
        // match is total, and answering its real registers keeps it so.
        Instruction::SendMirror {
            mirror_register,
            subject_register,
            argument_registers,
            destination,
            ..
        } => {
            let mut registers = argument_registers.clone();
            registers.push(*mirror_register);
            registers.push(*subject_register);
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
        Instruction::MakeVector { destination, .. } => vec![*destination],
        // `index` is a slot of the vector `Array`, not a register of this
        // frame -- checked at run time against the vector's actual length,
        // since a vector's size is a run-time value.
        Instruction::LoadVector {
            destination,
            vector,
            ..
        } => vec![*destination, *vector],
        Instruction::StoreVector { vector, source, .. } => vec![*vector, *source],
        Instruction::InitializeSlot { source, .. } => vec![*source],
        Instruction::ReturnSlotIfFilled { .. } => vec![],
        Instruction::MakeFuture {
            destination,
            closure,
        } => vec![*destination, *closure],
        Instruction::ResolveFuture {
            destination,
            source,
        } => vec![*destination, *source],
        Instruction::LoadOwnSlot { destination, .. } => vec![*destination],
        Instruction::InitializeSlotProvisionally { source, .. } => vec![*source],
        Instruction::Return { register }
        | Instruction::ImplicitReturn { register }
        | Instruction::NonLocalReturn { register }
        | Instruction::JumpIfNotBoolean { register, .. }
        | Instruction::BranchIfFalse { register, .. }
        | Instruction::BranchIfTrue { register, .. } => vec![*register],
        Instruction::Jump { .. } => vec![],
        Instruction::IntegerBinary {
            destination,
            left,
            right,
            ..
        } => vec![*destination, *left, *right],
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
        | Instruction::BranchIfTrue { target, .. }
        | Instruction::IntegerBinary { target, .. } => Some(*target),
        // Never decoded (it has no wire tag); its target is still one.
        Instruction::SendMirror { failure, .. } => Some(*failure),
        Instruction::LoadLiteral { .. }
        | Instruction::LoadSelf { .. }
        | Instruction::LoadEnclosingObject { .. }
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
        | Instruction::MakeVector { .. }
        | Instruction::LoadVector { .. }
        | Instruction::StoreVector { .. }
        | Instruction::InitializeSlot { .. }
        | Instruction::ReturnSlotIfFilled { .. }
        | Instruction::MakeFuture { .. }
        | Instruction::ResolveFuture { .. }
        | Instruction::LoadOwnSlot { .. }
        | Instruction::InitializeSlotProvisionally { .. }
        | Instruction::Return { .. }
        | Instruction::ImplicitReturn { .. }
        | Instruction::NonLocalReturn { .. } => None,
    }
}

fn verify_outer_locals(
    table: &DecodedCodeTable,
    parents: &[Option<usize>],
    index: usize,
    instructions: &[Instruction],
) -> Result<(), DecodeError> {
    for instruction in instructions {
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
    let copied_count = reader.read_u16()?;
    // The copied values' own half of the register layout. `argument_count`
    // alone exceeding the file is a different fault, reported as
    // `ArgumentsExceedRegisters` by `verify_body`, so this is asked only
    // once that holds -- and in `u32` arithmetic, so the sum cannot wrap
    // back into range.
    if argument_count <= register_count
        && u32::from(argument_count) + u32::from(copied_count) > u32::from(register_count)
    {
        return Err(DecodeError::CopiedCountOutOfRange {
            argument_count,
            copied_count,
            register_count,
        });
    }
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
    // Whether this body's sends were compiled uncached (`encode::encode_code`).
    // A number is no longer part of the artifact -- `decode_instruction`
    // builds every send with a placeholder, `CallSite::UNCACHED` for an
    // uncached body and `CallSite { number: 0 }` otherwise, and
    // `number_call_site_families` overwrites the placeholder with the real
    // number once every body in the table is read.
    let uncached = reader.read_boolean()?;
    let instruction_count = reader.read_u32()? as usize;
    let mut builder = CodeBuilder::new();
    for _ in 0..instruction_count {
        builder.emit(decode_instruction(reader, codes, symbols, uncached)?);
    }
    // `from_builder` derives `family_size` and `outer_reach` as the compiler's
    // bodies derive them, off the encoding. `family_size` is a placeholder, not
    // yet the real answer: every send here carries a number-shaped stand-in,
    // not the number `number_call_site_families` will give it, so this would
    // double-count a non-uncached body's sends as if they were already dense
    // from 0; it is recomputed there, once the real numbers exist.
    // `outer_reach`, never on the wire, reads each nested block's own, which is
    // already filled because post-order puts a block's code before its
    // parent's.
    Ok(CompiledCode::from_builder(
        kind,
        builder,
        argument_count,
        register_count,
        copied_count,
        statement_program_counter,
        source,
    ))
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

/// `uncached` is the enclosing code record's own flag
/// (`encode::compiled_uncached`): every send this decodes gets
/// `CallSite::UNCACHED` when it is set, or a `CallSite { number: 0 }`
/// placeholder otherwise, for `number_call_site_families` to overwrite once
/// the whole table is read.
fn decode_instruction(
    reader: &mut Reader,
    codes: &[Rc<CompiledCode>],
    symbols: &symbols::Table,
    uncached: bool,
) -> Result<Instruction, DecodeError> {
    let call_site = if uncached {
        CallSite::UNCACHED
    } else {
        CallSite { number: 0 }
    };
    Ok(match reader.read_u8()? {
        OPCODE_LOAD_LITERAL => {
            let destination = reader.read_u16()?;
            Instruction::LoadLiteral {
                destination,
                literal: decode_literal(reader, symbols)?,
            }
        }
        OPCODE_LOAD_SELF => Instruction::LoadSelf {
            destination: reader.read_u16()?,
        },
        OPCODE_LOAD_ENCLOSING_OBJECT => {
            let destination = reader.read_u16()?;
            Instruction::LoadEnclosingObject {
                destination,
                depth: reader.read_u16()?,
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
                call_site,
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
                call_site,
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
                call_site,
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
                call_site,
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
                copied: decode_registers(reader)?,
            }
        }
        OPCODE_MAKE_ARRAY => {
            let destination = reader.read_u16()?;
            Instruction::MakeArray {
                destination,
                element_registers: decode_registers(reader)?,
            }
        }
        OPCODE_MAKE_VECTOR => {
            let destination = reader.read_u16()?;
            Instruction::MakeVector {
                destination,
                size: reader.read_u16()?,
            }
        }
        OPCODE_LOAD_VECTOR => {
            let destination = reader.read_u16()?;
            let vector = reader.read_u16()?;
            Instruction::LoadVector {
                destination,
                vector,
                index: reader.read_u16()?,
            }
        }
        OPCODE_STORE_VECTOR => {
            let vector = reader.read_u16()?;
            let index = reader.read_u16()?;
            Instruction::StoreVector {
                vector,
                index,
                source: reader.read_u16()?,
            }
        }
        OPCODE_INITIALIZE_SLOT => {
            let source = reader.read_u16()?;
            Instruction::InitializeSlot {
                source,
                own_index: reader.read_u32()?,
            }
        }
        OPCODE_RETURN_SLOT_IF_FILLED => Instruction::ReturnSlotIfFilled {
            own_index: reader.read_u32()?,
        },
        OPCODE_MAKE_FUTURE => {
            let destination = reader.read_u16()?;
            Instruction::MakeFuture {
                destination,
                closure: reader.read_u16()?,
            }
        }
        OPCODE_RESOLVE_FUTURE => {
            let destination = reader.read_u16()?;
            Instruction::ResolveFuture {
                destination,
                source: reader.read_u16()?,
            }
        }
        OPCODE_LOAD_OWN_SLOT => {
            let destination = reader.read_u16()?;
            Instruction::LoadOwnSlot {
                destination,
                own_index: reader.read_u32()?,
            }
        }
        OPCODE_INITIALIZE_SLOT_PROVISIONALLY => {
            let source = reader.read_u16()?;
            Instruction::InitializeSlotProvisionally {
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
        OPCODE_INTEGER_BINARY => {
            let operation = read_integer_operation(reader)?;
            let destination = reader.read_u16()?;
            let left = reader.read_u16()?;
            let right = reader.read_u16()?;
            Instruction::IntegerBinary {
                operation,
                destination,
                left,
                right,
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

/// The inverse of `encode::write_large_integer`. Refuses every encoding but
/// the canonical one, so an integer has one encoding on the wire: one that
/// fits `i64` must travel as `VALUE_SMALL_INTEGER` (or
/// `LITERAL_SMALL_INTEGER`) -- fits `i64`, not 61 bits, because the wire's
/// canonical form is independent of where a `SmallInteger` ends -- and
/// neither a zero top digit nor a negative zero is written by the encoder.
fn read_large_integer(reader: &mut Reader) -> Result<ArbitraryInteger, DecodeError> {
    let (negative, magnitude) = read_large_integer_parts(reader)?;
    if magnitude.last() == Some(&0) || (magnitude.is_empty() && negative) {
        return Err(DecodeError::NonCanonicalInteger);
    }
    let integer = ArbitraryInteger::from_parts(negative, magnitude);
    if integer.to_i64().is_some() {
        return Err(DecodeError::NonCanonicalInteger);
    }
    Ok(integer)
}

/// A `LargeInteger`'s sign and digits as the wire has them, unchecked
/// beyond their shape: what `read_large_integer` validates, and what
/// `inspect`, a structural walk that refuses nothing canonical-encoding
/// related, renders as it is. The count is bounded against the bytes left
/// before anything is allocated for it.
pub(crate) fn read_large_integer_parts(
    reader: &mut Reader,
) -> Result<(bool, Vec<u64>), DecodeError> {
    let negative = match reader.read_u8()? {
        0 => false,
        1 => true,
        tag => {
            return Err(DecodeError::InvalidTag {
                what: "large integer sign",
                tag,
            })
        }
    };
    let count = bounded_count(reader.read_u32()? as usize, 8, reader)?;
    let mut magnitude = Vec::with_capacity(count);
    for _ in 0..count {
        magnitude.push(reader.read_u64()?);
    }
    Ok((negative, magnitude))
}

fn decode_literal(reader: &mut Reader, symbols: &symbols::Table) -> Result<Literal, DecodeError> {
    Ok(match reader.read_u8()? {
        LITERAL_NIL => Literal::Nil,
        LITERAL_BOOLEAN => Literal::Boolean(reader.read_boolean()?),
        LITERAL_SMALL_INTEGER => {
            let number = reader.read_i64()?;
            if fits_small_integer(number) {
                Literal::SmallInteger(number)
            } else {
                Literal::LargeInteger(ArbitraryInteger::from_i64(number))
            }
        }
        LITERAL_FLOAT => Literal::Float(reader.read_f64()?),
        LITERAL_STRING => Literal::String(reader.read_str()?),
        LITERAL_SYMBOL => Literal::Symbol(decode_symbol(reader, symbols)?),
        LITERAL_UNRESOLVED_IDENTIFIER => Literal::UnresolvedIdentifier(reader.read_str()?),
        LITERAL_LARGE_INTEGER => Literal::LargeInteger(read_large_integer(reader)?),
        LITERAL_BODY_TOO_LARGE => Literal::BodyTooLarge,
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
/// (`mutator`), the module classes built from the mixin table, the decoded
/// code table, the interpreter's `platform` singleton, and the decoded
/// declaration trees a class object's mixin id names.
///
/// `mutator` is `&mut` because decoding a class object *allocates into* the
/// interpreter: rebuilding its `MixinApplication` goes through
/// `Mutator::apply_mixin`, and that shared cache is the whole mechanism
/// by which two enclosing instances get back the one application they had
/// before the snapshot.
pub struct DecodeContext<'a, 'gc> {
    pub mutator: &'a mut Mutator<'gc>,
    /// Each module's root class and `<:` levels, lowest first, as
    /// `modules::build_module_classes` built them: what a `CLASS_MODULE` and
    /// a `CLASS_MODULE_LEVEL` class tag name by index.
    pub modules: &'a ModuleClasses<'gc>,
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
    /// What each entry of the image's platform path section answered when
    /// `load` sent its members to the loading platform, by index: what a
    /// `VALUE_PLATFORM_PATH`, a `CLASS_PLATFORM_PATH` and a
    /// `HASHED_PLATFORM_PATH` name. Resolved before the graph is decoded, so
    /// the decoder only looks them up and runs no code.
    pub platform_paths: &'a [Value<'gc>],
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

/// The two placeholder-allocated tables, indexed by the ids in the stream.
pub struct Tables<'a, 'gc> {
    pub objects: &'a [ObjectReference<'gc>],
    pub frames: &'a [FrameReference<'gc>],
}

/// The inverse of `encode::encode_class_tag`. Tag 2's object may still be a
/// placeholder when this runs -- a class object can be named before its own
/// record is read -- so this resolves identity only; that the record really
/// was a class record is `check_class_references`'s job, once every record
/// has been read. Tag 4 names the metaclass of a class the loading world
/// already has, which exists by then: `build_root_class` builds the root
/// and its levels through `Mutator::new_class_object` and
/// `new_applied_level`, which attach one, after the bootstrap attached the
/// builtins'.
pub fn decode_class_tag<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
) -> Result<ClassReference<'gc>, DecodeError> {
    let tag = reader.read_u8()?;
    decode_class_tag_after(tag, reader, context, tables)
}

/// An object record's header, as `decode_object_header` read it.
struct ObjectHeader<'gc> {
    class: ClassReference<'gc>,
    /// The header named the builtin `Activation`.
    activation: bool,
    /// The header named the builtin `Future`.
    future: bool,
    /// The header named a class by platform path (`CLASS_PLATFORM_PATH`),
    /// whose layout comes from the loading binary.
    platform_path: bool,
}

/// An object record's header: its class tag, which is `decode_class_tag`'s
/// except that it also accepts the builtin `Activation` -- the header of an
/// activation record, the one place an image may name that class -- and the
/// builtin `Future`, the header of an unresolved future's record, for the
/// same reason. Answers whether it did, so the caller can hold the header
/// and the body to each other (`DecodeError::ActivationHeader`,
/// `DecodeError::FutureRecord`) once it has read the body, and whether it
/// named its class by platform path, so the caller can hold an instance's
/// fields to that class's layout.
fn decode_object_header<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
) -> Result<ObjectHeader<'gc>, DecodeError> {
    let tag = reader.read_u8()?;
    if tag != CLASS_BUILTIN {
        return Ok(ObjectHeader {
            class: decode_class_tag_after(tag, reader, context, tables)?,
            activation: false,
            future: false,
            platform_path: tag == CLASS_PLATFORM_PATH,
        });
    }
    let kind = match read_builtin_class(reader)? {
        kind @ (BuiltinClass::Activation | BuiltinClass::Future) => kind,
        kind => referenced_builtin_class(kind)?,
    };
    Ok(ObjectHeader {
        class: context.mutator.builtin_class(kind),
        activation: kind == BuiltinClass::Activation,
        future: kind == BuiltinClass::Future,
        platform_path: false,
    })
}

/// `decode_class_tag` once its tag byte has been read.
fn decode_class_tag_after<'gc>(
    tag: u8,
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
) -> Result<ClassReference<'gc>, DecodeError> {
    match tag {
        CLASS_BUILTIN => Ok(context
            .mutator
            .builtin_class(read_referenced_builtin_class(reader)?)),
        CLASS_RECORD => decode_object_id(reader, tables),
        CLASS_MODULE => read_module_class(reader, context.modules),
        CLASS_MODULE_LEVEL => read_module_level(reader, context.modules),
        CLASS_PLATFORM_PATH => {
            read_platform_path_class(&context.mutator.heap, reader, context.platform_paths)
        }
        CLASS_METACLASS => {
            let class = read_prebuilt_class(
                context.mutator,
                reader,
                context.modules,
                context.platform_paths,
            )?;
            context
                .mutator
                .heap
                .class_metaclass(class)
                .ok_or(DecodeError::NotAClassReference)
        }
        tag => Err(DecodeError::InvalidTag { what: "class", tag }),
    }
}

/// The class a `CLASS_METACLASS` reference or a `HASHED_METACLASS` entry
/// names the metaclass of: a builtin, a module's root class, a module level
/// or a class named by platform path -- a class the loading world has before
/// the graph. A second `CLASS_METACLASS` or a `CLASS_RECORD` here is refused,
/// which bounds the nesting at one: a class record's metaclass is a record of
/// its own and is named by its id.
fn read_prebuilt_class<'gc>(
    mutator: &Mutator<'gc>,
    reader: &mut Reader,
    modules: &ModuleClasses<'gc>,
    platform_paths: &[Value<'gc>],
) -> Result<ClassReference<'gc>, DecodeError> {
    match reader.read_u8()? {
        CLASS_BUILTIN => Ok(mutator.builtin_class(read_referenced_builtin_class(reader)?)),
        CLASS_MODULE => read_module_class(reader, modules),
        CLASS_MODULE_LEVEL => read_module_level(reader, modules),
        CLASS_PLATFORM_PATH => read_platform_path_class(&mutator.heap, reader, platform_paths),
        tag => Err(DecodeError::InvalidTag {
            what: "prebuilt class",
            tag,
        }),
    }
}

/// A platform path reference's index, after its tag, resolved against what
/// `load` resolved the section's paths to: in range, or `BadPlatformPath`
/// before anything indexes by it.
fn read_platform_path<'gc>(
    reader: &mut Reader,
    platform_paths: &[Value<'gc>],
) -> Result<Value<'gc>, DecodeError> {
    let index = reader.read_u32()?;
    platform_paths
        .get(index as usize)
        .copied()
        .ok_or(DecodeError::BadPlatformPath(index))
}

/// `read_platform_path` where the format types the reference as a class:
/// its object must be a class object.
fn read_platform_path_class<'gc>(
    heap: &Heap<'gc>,
    reader: &mut Reader,
    platform_paths: &[Value<'gc>],
) -> Result<ClassReference<'gc>, DecodeError> {
    match read_platform_path(reader, platform_paths)?.as_object() {
        Some(class) if heap.is_class(class) => Ok(class),
        _ => Err(DecodeError::PlatformPathNotAClass),
    }
}

/// A `CLASS_MODULE`'s module index, after its tag, resolved against the
/// module classes `load` rebuilt: in range, or `BadModuleReference` before
/// anything indexes by it.
fn read_module_class<'gc>(
    reader: &mut Reader,
    modules: &ModuleClasses<'gc>,
) -> Result<ClassReference<'gc>, DecodeError> {
    let module = reader.read_u32()?;
    modules
        .root(module)
        .ok_or(DecodeError::BadModuleReference { module })
}

/// A `CLASS_MODULE_LEVEL`'s module index and position, after its tag,
/// resolved the same way.
fn read_module_level<'gc>(
    reader: &mut Reader,
    modules: &ModuleClasses<'gc>,
) -> Result<ClassReference<'gc>, DecodeError> {
    let module = reader.read_u32()?;
    let position = reader.read_u32()?;
    modules
        .level(module, position)
        .ok_or(DecodeError::BadModuleLevel { module, position })
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
    context: &mut DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
) -> Result<Value<'gc>, DecodeError> {
    match reader.read_u8()? {
        VALUE_NIL => Ok(Value::nil()),
        VALUE_FALSE => Ok(Value::boolean(false)),
        VALUE_TRUE => Ok(Value::boolean(true)),
        // An `i64` beyond 61 bits allocates its `LargeInteger`, which adds
        // to the load's peak as `VALUE_LARGE_INTEGER` does (below), and more
        // per wire byte: 9 bytes (tag, `i64`) for an `Object` and one digit,
        // 64/9 = 7.1x in a release build and 80/9 = 8.9x in a debug one, on
        // top of the placeholders' 16.0x and 20.0x: about 23.1x and 28.9x
        // in all, the worst case of a rejected load.
        VALUE_SMALL_INTEGER => Ok(integer_value(context.mutator, reader.read_i64()?)),
        // A float outside the immediate range allocates its boxed `Float`:
        // 9 bytes (tag, `f64`) for an `Object` alone, its payload inline,
        // 56/9 = 6.2x in a release build and 72/9 = 8.0x in a debug one, on
        // top of the placeholders: about 22.2x and 28.0x, below
        // `VALUE_SMALL_INTEGER`'s, which stays the worst case.
        VALUE_FLOAT => Ok(context.mutator.new_float(reader.read_f64()?)),
        VALUE_OBJECT_ID => Ok(Value::object(decode_object_id(reader, tables)?)),
        VALUE_CLASS_TAG => Ok(Value::object(decode_class_tag(reader, context, tables)?)),
        VALUE_PLATFORM => Ok(context.platform),
        VALUE_PLATFORM_PATH => read_platform_path(reader, context.platform_paths),
        // Allocating here is sound: the decoder never polls a safepoint, so
        // nothing moves the placeholders `tables` holds.
        //
        // It adds to the load's peak rather than fitting inside it. The
        // placeholders are allocated first, against every byte left at the
        // graph section (`decode_graph_section`), and those same bytes can
        // then be one array record full of these values, each allocating
        // its `Object` and its digits before a later record fails to read.
        // A value is at least 14 bytes on the wire (tag, sign, count, one
        // digit) and costs `size_of::<Object>()` plus 8 bytes a digit. With
        // an `Object` of 56 bytes in a release build and 72 in a debug one,
        // that is 64/14 = 4.6x and 80/14 = 5.7x of those bytes, on top of
        // the placeholders' 9.3x + 6.7x (release) and 12.0x + 8.0x (debug):
        // about 20.6x and 25.7x in all, below `VALUE_SMALL_INTEGER`'s
        // worst case (above). It stays proportional to the input
        // because the digit count is bounded against the bytes left before
        // anything is allocated for it (`read_large_integer_parts`), and each
        // further digit costs its own 8 wire bytes.
        VALUE_LARGE_INTEGER => {
            let integer = read_large_integer(reader)?;
            Ok(context.mutator.new_integer(integer))
        }
        tag => Err(DecodeError::InvalidTag { what: "value", tag }),
    }
}

fn decode_values<'gc>(
    reader: &mut Reader,
    context: &mut DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
) -> Result<Vec<Value<'gc>>, DecodeError> {
    let count = reader.read_u32()? as usize;
    let mut values = reserve_for(count, reader);
    for _ in 0..count {
        values.push(decode_value(reader, context, tables)?);
    }
    Ok(values)
}

/// `decode_values` for what a heap object stores: each value goes straight
/// into its word, so a load holds no second list beside the first.
fn decode_words<'gc>(
    reader: &mut Reader,
    context: &mut DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
) -> Result<Vec<Word>, DecodeError> {
    let count = reader.read_u32()? as usize;
    let mut words = reserve_for(count, reader);
    for _ in 0..count {
        let value = decode_value(reader, context, tables)?;
        words.push(context.mutator.heap.store(value));
    }
    Ok(words)
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
    context: &mut DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
) -> Result<ClosureData<'gc>, DecodeError> {
    let code_id = reader.read_u32()?;
    let code = context
        .codes
        .get(code_id as usize)
        .cloned()
        .ok_or(DecodeError::BadCodeId(code_id))?;
    let captured_frame = decode_frame_id(reader, tables)?;
    let count = reader.read_u16()? as usize;
    // The one place in the format that can see both halves of a closure's
    // copied values. Checked before any of them is read, so a mismatch is
    // reported as itself rather than as whatever the next field trips on.
    if count != code.copied_count as usize {
        return Err(DecodeError::CopiedCountMismatch {
            copied: count,
            copied_count: code.copied_count,
        });
    }
    let mut copied = reserve_for(count, reader);
    for _ in 0..count {
        let value = decode_value(reader, context, tables)?;
        copied.push(context.mutator.heap.store(value));
    }
    Ok(ClosureData {
        code,
        captured_frame,
        copied: copied.into_boxed_slice(),
        // Never on the wire: a decoded closure is given a fresh range of its
        // family's size when it is first activated (`Mutator::push_step`).
        feedback_base: crate::interpreter::FeedbackBase::NOT_ASSIGNED,
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
/// `None` -- a `TopLevel` activation -- has no class to ask, so the
/// application's module, module 0, stands in, as its root class did for
/// every frame before nesting.
fn method_declaration<'gc>(
    context: &DecodeContext<'_, 'gc>,
    current_class: Option<&ClassReference<'gc>>,
    selector: Symbol,
) -> Result<Rc<crate::mixin::MixinMethod>, DecodeError> {
    let class = match current_class {
        Some(class) => *class,
        None => context.modules.root(0).ok_or(DecodeError::NotAClass)?,
    };
    let found = context
        .mutator
        .heap
        .class_application(class)
        .and_then(|application| application.mixin.compiled_method(selector).cloned());
    found.ok_or(DecodeError::UnknownSelector(selector.as_str().to_string()))
}

fn decode_frame<'gc>(
    reader: &mut Reader,
    context: &mut DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    symbols: &symbols::Table,
) -> Result<Frame<'gc>, DecodeError> {
    let receiver = decode_value(reader, context, tables)?;
    let receiver = context.mutator.heap.store(receiver);
    let arguments = decode_words(reader, context, tables)?;
    let temporaries = decode_words(reader, context, tables)?;
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
        FRAME_MIXIN_APPLICATION_MESSAGE => {
            FrameKind::MixinApplicationMessage(decode_class_tag(reader, context, tables)?)
        }
        FRAME_CLOSURE => FrameKind::Closure(decode_object_id(reader, tables)?),
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
    })
}

/// A safe, fully initialized stand-in, overwritten in place by
/// `decode_graph_section`'s second pass before anything can observe it.
fn placeholder_frame<'gc>() -> Frame<'gc> {
    Frame {
        receiver: Word::NIL,
        arguments: Vec::new(),
        temporaries: Vec::new(),
        program_counter: 0,
        sender: None,
        home: None,
        current_class: None,
        kind: FrameKind::TopLevel,
        uncontinuable: false,
    }
}

/// A floor under the smallest object record the wire admits: a class tag of
/// at least one byte (every live one is followed by at least one more), an
/// identity hash (4), and a body tag (1). Every body adds at least four more
/// bytes, so this is a floor, not a typical size; kept at the one-byte tag
/// the retired `CLASS_USER_ROOT` was, since the load peaks `memory.md`
/// reports are derived from it and a lower bound stays a sound one.
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

/// Module `module_index`'s root class object, rebuilt from the first row of
/// its tree in the mixin table and its root section --
/// `instantiate::load_module` without the source.
///
/// The two do the same three things in the same order, so a loaded world's
/// root class is indistinguishable from a freshly loaded module's, down to
/// its identity hash (both allocate the same class objects in the same
/// order, module after module, after the bootstrap's builtins): apply the
/// root mixin over the
/// superclass's own application, then mint the class object with no
/// enclosing object and no declaring class -- the module's root class is
/// the outermost level, so neither exists.
///
/// The superclass arrives as an already-resolved `ClassReference` from
/// `decode_root_superclass`, because `load_module` resolves it by *name*
/// from the declaration and a decoded mixin has no declaration to read a
/// name from.
///
/// The root's `<:` levels (`newspeak-spec.pdf` §6.2) are built here too, by
/// `nested::build_applied_levels` -- the function `load_module` builds them
/// with -- and in the same place in the order, before the root class object
/// itself. Answers the root class and its levels, lowest first: the levels
/// are what a `CLASS_MODULE_LEVEL` class tag names by position.
pub fn build_root_class<'gc>(
    mutator: &mut Mutator<'gc>,
    mixins: &mixins::Decoded,
    module_index: usize,
    section: &RootSection,
) -> Result<(ClassReference<'gc>, Vec<ClassReference<'gc>>), DecodeError> {
    let root_mixin = mixins
        .module_root(module_index)
        .cloned()
        .ok_or(DecodeError::NotAClass)?;
    let builtin = mutator.builtin_class(section.superclass);
    let superclass = mutator
        .heap
        .class_application(builtin)
        .map(|application| (builtin, application));
    let applied: Vec<(usize, Value<'gc>)> = section
        .levels
        .iter()
        .map(|&(index, kind)| (index as usize, Value::object(mutator.builtin_class(kind))))
        .collect();
    // `decode_root_section` held every index to the root row's clauses, and a
    // builtin class is always a class object, so this cannot fail; the
    // mapping keeps a future change to either from being a panic.
    let top = crate::interpreter::nested::build_applied_levels(
        mutator,
        &root_mixin,
        superclass,
        &applied,
    )
    .map_err(|_| DecodeError::NotAClassReference)?;
    let (superclass, super_application) = top.unzip();
    let application = mutator.apply_mixin(&root_mixin, super_application.as_ref());
    let root_class = mutator.new_class_object(application, None, None, superclass);
    let levels = super::encode::root_levels(&mutator.heap, root_class);
    Ok((root_class, levels))
}

/// A module's root section as read: the builtin its root class inherits from
/// -- `Object` or `Value`, the only two the root rule allows -- and the
/// root's levels, lowest first, each as the position of the `<:` clause in
/// the module's root row that added it and the builtin that clause named.
#[derive(Debug, PartialEq)]
pub struct RootSection {
    pub superclass: BuiltinClass,
    pub levels: Vec<(u32, BuiltinClass)>,
}

/// Module `module_index`'s root section: the class its root class inherits
/// from, as a builtin tag behind a presence flag, then the levels its `<:`
/// clauses add. The inverse of `encode::encode_root_class`, which explains
/// why the format admits only builtins here.
///
/// Every level's clause position is checked against the clauses of this
/// module's own root row -- never another module's -- and must rise from one
/// level to the next: the levels are built in clause order, and no clause
/// adds two. That bounds the count by the row's clause count before anything
/// is reserved for it. Resolving no tag against a live interpreter, this
/// serves `inspect` as well as `load`.
///
/// The reader refuses what the root rule forbids
/// (`resolver::root_clause_defect`) -- a missing superclass,
/// a superclass other than `Object` or `Value`, a level naming anything but
/// `Value`, a second level, a level over anything but `Object` -- so a
/// crafted artifact cannot bring back a root the encoder would refuse.
pub fn decode_root_section(
    reader: &mut Reader,
    mixins: &mixins::Decoded,
    module_index: usize,
) -> Result<RootSection, DecodeError> {
    if !reader.read_boolean()? {
        return Err(DecodeError::RootWithoutSuperclass);
    }
    let superclass = read_program_builtin_class(reader)?;
    if !matches!(superclass, BuiltinClass::Object | BuiltinClass::Value) {
        return Err(DecodeError::InvalidTag {
            what: "builtin class a module's root may inherit",
            tag: super::encode::builtin_class_tag(superclass),
        });
    }
    let clause_count = mixins
        .module_root(module_index)
        .map_or(0, |root| root.mixin_applications.len());
    let row = mixins
        .module_rows
        .get(module_index)
        .copied()
        .unwrap_or_default();
    let count = reader.read_u32()? as usize;
    let mut levels = Vec::with_capacity(count.min(clause_count));
    let mut next_allowed: u32 = 0;
    for _ in 0..count {
        let index = reader.read_u32()?;
        if index < next_allowed
            || index as usize >= clause_count
            || !levels.is_empty()
            || superclass != BuiltinClass::Object
        {
            return Err(DecodeError::BadMixinApplicationClause { row, index });
        }
        next_allowed = index + 1;
        let kind = read_program_builtin_class(reader)?;
        if kind != BuiltinClass::Value {
            return Err(DecodeError::InvalidTag {
                what: "builtin class a module's root may apply",
                tag: super::encode::builtin_class_tag(kind),
            });
        }
        levels.push((index, kind));
    }
    Ok(RootSection { superclass, levels })
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
/// A metaclass record is applied first, over `Class`: `newspeak-spec.pdf`
/// §6 makes a class's metaclass "the application of the metaclass mixin
/// defined by the class' class side to the class Class", which is what the
/// live `Mutator::attach_metaclass` builds, and both halves are known once
/// every record is read -- the class record's mixin and `Class`'s
/// application -- so nothing orders it after the class records. Applying it
/// first means a class record whose superclass is a metaclass record (which
/// no real world has, but nothing in the stream forbids) is applied over the
/// metaclass's real application rather than over the placeholder. The
/// pairing of every class record with its metaclass record is checked after
/// the class records are applied (`pair_metaclasses`).
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
    pending_metaclasses: &[usize],
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
    rebuild_metaclasses(context, objects, &pending, &position, pending_metaclasses)?;
    let mut done = vec![false; pending.len()];
    let mut on_chain = vec![false; pending.len()];
    let mut chain: Vec<usize> = Vec::new();
    // Recorded in the order each class's application is installed --
    // superclass first, by construction of the loop below -- and replayed
    // by the second pass afterward. §3.1.1's enclosing-object condition
    // reads a level's *enclosing* class's application too
    // (`is_a_value`, `Heap::describe`), which is a different axis than the
    // superclass chain this loop orders: an enclosing class can be any
    // other pending record, done or not, at the point a superclass chain
    // reaches it. Recomputing a refusal inside this loop, right after its
    // own application installs, read a not-yet-installed enclosing class's
    // placeholder application instead of its real one -- answering `Object`
    // as its name and, had its real answer to `inherits_value` differed
    // from the placeholder's `false`, potentially the wrong verdict, not
    // only the wrong text. Running the recompute only once every class in
    // `pending` already carries its real application avoids both.
    let mut installed_order: Vec<usize> = Vec::with_capacity(pending.len());
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
            let object = *object;
            if !context
                .mutator
                .heap
                .class_install_application(object, application)
            {
                unreachable!("this object was filled in as a class record");
            }
            installed_order.push(pending_index);
            done[pending_index] = true;
            on_chain[pending_index] = false;
        }
        chain.clear();
    }
    pair_metaclasses(
        &context.mutator.heap,
        objects,
        &pending,
        pending_metaclasses,
    )?;
    // §3.1.1's enclosing-object condition is decided when the class object
    // is created (`interpreter::value_class`), never reconstructed from the
    // wire -- a decoded record starts with no verdict (`ClassData::new`
    // defaults `None`). A legitimate snapshot can hold a class that was
    // created carrying one (or one built of decoded pieces would carry,
    // `Counter`-over-`Holder`-style), so it is recomputed here, exactly as
    // `after_clause` decides it for a class created from source -- never for
    // a metaclass or an applied level, neither of which `after_clause`
    // checks either, and only when the application inherits `Value`,
    // matching its own guard. A class with no lexical enclosing -- a module
    // root, a builtin -- never reaches `after_clause` and is skipped the
    // same way here. The identity-method condition needs no recompute of
    // its own: it is a property of the application, already folded into
    // `instantiation_refusal` by the `apply_mixin` call in the loop above,
    // for a root's application exactly as for any other's.
    //
    // A second pass, not folded into the loop above: `installed_order`'s
    // own doc comment says why every application has to be real first.
    // Still superclass first, since that is the order `installed_order`
    // was recorded in.
    for pending_index in installed_order {
        let object = objects[pending[pending_index].object];
        let application = context
            .mutator
            .heap
            .class_application(object)
            .expect("every pending class was installed above");
        if !application.inherits_value || context.mutator.heap.class_is_applied_level(object) {
            continue;
        }
        let (Some(enclosing), Some(superclass_class)) = (
            context.mutator.heap.class_enclosing(object),
            context.mutator.heap.class_superclass(object),
        ) else {
            continue;
        };
        if let Some(refusal) = crate::interpreter::value_class::check_value_class(
            context.mutator,
            enclosing,
            superclass_class,
        ) {
            context
                .mutator
                .heap
                .class_set_value_refusal(object, refusal);
        }
    }
    Ok(())
}

/// Applies every metaclass record over `Class`, as `Mutator::attach_metaclass`
/// applies a live one: the metaclass mixin of its class's row
/// (`Mutator::metaclass_mixin`, the row's class side or the memoized mixin
/// holding only the factory) over `Class`'s application, with its class's
/// enclosing object and declaring class, since §6.3 writes a class side
/// inside the same declaration as its instance side.
///
/// A metaclass record's class must be a class record: `position` holds
/// exactly those. A metaclass of a builtin, the root class or a root level
/// is named by tag (`CLASS_METACLASS`) and already exists, and one of a
/// metaclass or of an ordinary object does not exist at all.
fn rebuild_metaclasses<'gc>(
    context: &mut DecodeContext<'_, 'gc>,
    objects: &[ObjectReference<'gc>],
    pending: &[PendingClass],
    position: &HashMap<usize, usize>,
    pending_metaclasses: &[usize],
) -> Result<(), DecodeError> {
    let class_class = context.mutator.builtin_class(BuiltinClass::Class);
    let class_application = context.mutator.heap.class_application(class_class);
    for &index in pending_metaclasses {
        let metaclass = objects[index];
        let this_class = context
            .mutator
            .heap
            .class_this_class(metaclass)
            .ok_or(DecodeError::MetaclassPairing)?;
        let key = context.mutator.heap.object_identity_key(this_class);
        let Some(&this_position) = position.get(&key) else {
            return Err(DecodeError::MetaclassPairing);
        };
        // `position` is keyed from `pending`, which holds only `OBJECT_CLASS`
        // records (`decode_class_record` never sets `this_class`), so this
        // can never fire through the lookup above -- an untrusted image is
        // decoded input, so the check stays as a second, independent guard
        // of the same invariant rather than relying only on `position`'s
        // construction elsewhere in this file.
        if context.mutator.heap.class_this_class(this_class).is_some() {
            return Err(DecodeError::MetaclassPairing);
        }
        let class_side_mixin = context
            .mutator
            .metaclass_mixin(&pending[this_position].mixin);
        let application = context
            .mutator
            .apply_mixin(&class_side_mixin, class_application.as_ref());
        let heap = &mut context.mutator.heap;
        let enclosing = heap.class_enclosing(this_class);
        let declaring_class = heap.class_declaring(this_class);
        if !heap.class_install_application(metaclass, application)
            || !heap.class_set_lexical_context(metaclass, enclosing, declaring_class)
        {
            unreachable!("this object was filled in as a metaclass record");
        }
    }
    Ok(())
}

/// Holds every class record and its metaclass record to each other, both
/// ways: a class record's header -- its class, where `Heap::class_metaclass`
/// reads it -- names a metaclass whose reference back is that class record,
/// and every metaclass record is named by exactly one class record's header.
///
/// Anything else is `MetaclassPairing`. A metaclass named by no class record
/// would be a second metaclass beside the one its class answers -- the
/// identity a round trip exists to keep -- and a class record naming another
/// class's metaclass would answer that class's class side.
fn pair_metaclasses<'gc>(
    heap: &Heap<'gc>,
    objects: &[ObjectReference<'gc>],
    pending: &[PendingClass],
    pending_metaclasses: &[usize],
) -> Result<(), DecodeError> {
    let mut named: HashMap<usize, usize> = HashMap::with_capacity(pending_metaclasses.len());
    for pending_class in pending {
        let class = objects[pending_class.object];
        if heap.class_metaclass(class).is_none() {
            return Err(DecodeError::MetaclassPairing);
        }
        let metaclass = heap.object_class(class);
        *named
            .entry(heap.object_identity_key(metaclass))
            .or_insert(0) += 1;
    }
    for &index in pending_metaclasses {
        if named.get(&heap.object_identity_key(objects[index])) != Some(&1) {
            return Err(DecodeError::MetaclassPairing);
        }
    }
    Ok(())
}

/// The mixin keys of the rows that are some other row's class side -- what
/// a class record may *not* apply: a class side is applied only by a
/// metaclass, which is a record of its own (`OBJECT_METACLASS`) whose
/// application follows from its class (`rebuild_metaclasses`).
///
/// Derived from the decoded table rather than from anything the graph
/// section writes, so a crafted class record cannot slip a class side past
/// the check by claiming to be something else. `mixins::check_preorder` has
/// already established that every row is reached exactly once, so no row is
/// the class side of two declarations.
fn metaclass_rows(mixins: &[Rc<crate::mixin::Mixin>]) -> HashSet<usize> {
    mixins
        .iter()
        .filter_map(|mixin| mixin.class_side.as_ref())
        .map(crate::mixin::mixin_key)
        .collect()
}

/// Every field the format types as a class reference really names a class
/// object, and a class object's own `class` is its metaclass or, for a
/// metaclass, `Metaclass`.
///
/// A tag-2 class tag names an object-table id, and nothing in the byte
/// stream forces that record to have been a class record -- which is only
/// knowable once every record has been read, so it is checked here rather
/// than where each tag is decoded. These five positions are the complete
/// set: a class tag appears nowhere else in the graph section, and a plain
/// object value pointing at a class object is legitimate (that is what a
/// cached nested class in a slot *is*).
///
/// The extra rule for a class object's *own* `class` is the runtime's
/// invariant (§3.2), not a limit invented here: a class record's header is
/// its metaclass record, whose reference back is it, and a metaclass
/// record's header is the builtin `Metaclass` (`MetaclassPairing`
/// otherwise), as `Mutator::attach_metaclass` gives every live class
/// object. So the chain from a class record to a class the graph did not
/// create is at most two links -- class record, its metaclass record,
/// `Metaclass` -- and that is what bounds it: no crafted image can build a
/// chain of class records each the class of the next.
///
/// An object that is not a class object may not be an instance of a class
/// whose chain reaches `Behavior` (`InstanceOfAClassClass`): `Class`,
/// `Metaclass` and every metaclass refuse instantiation, so only a crafted
/// image has one. A metaclass record's reference back to its class is a
/// class object's by the time this runs -- `rebuild_metaclasses` refused
/// anything else.
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
) -> Result<(), DecodeError> {
    for object in objects {
        let class = heap.object_class(*object);
        if !heap.is_class(class) {
            return Err(DecodeError::NotAClassReference);
        }
        if !heap.is_class(*object) {
            let class_object_class = heap.class_application(class).is_some_and(|application| {
                matches!(
                    application.instantiation_refusal,
                    Some(crate::mixin::InstantiationRefusal::ClassObject)
                )
            });
            if class_object_class {
                return Err(DecodeError::InstanceOfAClassClass);
            }
        }
        if heap.is_class(*object) {
            // The decoding loop and `pair_metaclasses` have refused every
            // header this could refuse; it is restated here, beside the
            // other invariants of a class object's references, because it
            // is what bounds the header chain at two links.
            let paired = match heap.class_this_class(*object) {
                Some(_) => heap.class_builtin(class) == Some(BuiltinClass::Metaclass),
                None => heap.class_metaclass(*object).is_some(),
            };
            if !paired {
                return Err(DecodeError::MetaclassPairing);
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
            | FrameKind::SuperclassMessage(class)
            | FrameKind::MixinApplicationMessage(class) => Some(class),
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
/// no part of this decoder recurses on a depth the image controls.
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
    let mut pending_metaclasses: Vec<usize> = Vec::new();
    let mut pending_futures: Vec<Value<'gc>> = Vec::new();
    let metaclass_rows = metaclass_rows(context.mixins);
    let metaclass_class = context.mutator.builtin_class(BuiltinClass::Metaclass);
    for (index, object) in objects.iter().enumerate() {
        let ObjectHeader {
            class,
            activation: activation_header,
            future: future_header,
            platform_path: class_named_by_path,
        } = decode_object_header(reader, context, &tables)?;
        let identity_hash = reader.read_u32()?;
        // Checked here, the one place identity hashes are read: `load`
        // hands the maximum straight to `advance_identity_hash_past`, which
        // must be able to allocate past it.
        if identity_hash == u32::MAX {
            return Err(DecodeError::IdentityHashExhausted);
        }
        max_identity_hash = max_identity_hash.max(identity_hash);
        let body = reader.read_u8()?;
        if activation_header != (body == OBJECT_ACTIVATION) {
            return Err(DecodeError::ActivationHeader);
        }
        if future_header && body != OBJECT_INSTANCE {
            return Err(DecodeError::FutureRecord);
        }
        let data = match body {
            OBJECT_INSTANCE => {
                let fields = decode_words(reader, context, &tables)?;
                // The loading binary lays a shipped class out, not the image,
                // so an instance of one must fit that layout exactly.
                if class_named_by_path {
                    let expected = context
                        .mutator
                        .heap
                        .class_application(class)
                        .map_or(0, |application| application.slot_count);
                    if fields.len() != expected {
                        return Err(DecodeError::PlatformInstanceFieldCount {
                            expected,
                            found: fields.len(),
                        });
                    }
                }
                // Held to what the encoder writes; that its computation is
                // a closure is checked below, once the record it names has
                // been read.
                if future_header {
                    let computation = crate::interpreter::future::decoded_future_computation(
                        &context.mutator.heap,
                        &fields,
                    )
                    .ok_or(DecodeError::FutureRecord)?;
                    pending_futures.push(computation);
                }
                ObjectData::Instance(fields)
            }
            OBJECT_STRING => ObjectData::String(reader.read_code_points()?),
            OBJECT_SYMBOL => ObjectData::Symbol(decode_symbol(reader, symbols)?),
            OBJECT_ARRAY => ObjectData::Array(decode_words(reader, context, &tables)?),
            OBJECT_CLOSURE => ObjectData::Closure(decode_closure_record(reader, context, &tables)?),
            // One frame id, resolved like a closure's captured frame: every
            // frame was placeholder-allocated before this pass, so a forward
            // reference is ordinary and an id past the table is `BadFrameId`.
            OBJECT_ACTIVATION => ObjectData::Activation(decode_frame_id(reader, &tables)?),
            OBJECT_CLASS => {
                let (data, mixin) =
                    decode_class_record(reader, context, &tables, &placeholder_application)?;
                if metaclass_rows.contains(&crate::mixin::mixin_key(&mixin)) {
                    return Err(DecodeError::ClassRecordAppliesAClassSide);
                }
                pending_classes.push(PendingClass {
                    object: index,
                    mixin,
                });
                // The header is stored as read: it names the record's
                // metaclass record, which `pair_metaclasses` holds to it
                // once every record is read.
                ObjectData::Class(Box::new(data))
            }
            OBJECT_METACLASS => {
                // §3.2: every metaclass is an instance of `Metaclass`.
                if !class.same(metaclass_class) {
                    return Err(DecodeError::MetaclassPairing);
                }
                let this_class = decode_class_tag(reader, context, &tables)?;
                let class_class = context.mutator.builtin_class(BuiltinClass::Class);
                let mut data = crate::types::ClassData::new(
                    placeholder_application.clone(),
                    None,
                    None,
                    Some(class_class),
                    None,
                );
                data.this_class = Some(this_class);
                pending_metaclasses.push(index);
                ObjectData::Class(Box::new(data))
            }
            tag => {
                return Err(DecodeError::InvalidTag {
                    what: "object body",
                    tag,
                })
            }
        };
        let decoded_symbol = match &data {
            ObjectData::Symbol(symbol) => Some(*symbol),
            _ => None,
        };
        context.mutator.heap.object_replace(
            *object,
            Object {
                class,
                identity_hash,
                // `class` may name a record later in this same table --
                // decoding fills records in table order, not dependency
                // order -- so that class may not have an application yet.
                // `NONE` matches no cache entry; `stamp_dispatch_ids`, right
                // after `rebuild_applications` gives every class one,
                // replaces it with the real id.
                dispatch_id: DispatchId::NONE,
                data,
            },
        );
        // A heap holds one object per symbol, and so does an image of one:
        // a second record of the same text is refused, not merged
        // (`docs/design/image-format.md`, "Verification"). `load` decodes
        // into a fresh interpreter, where nothing made a symbol before the
        // graph section, so an entry already here came from this graph.
        //
        // Checked here too, before the record enters the canonical table
        // below: a symbol record whose class tag is not the builtin
        // `Symbol` class would still answer every later literal of that
        // text once entered, so it is refused rather than entered under
        // the wrong class.
        if let Some(symbol) = decoded_symbol {
            if !class.same(context.mutator.builtin_class(BuiltinClass::Symbol)) {
                return Err(DecodeError::SymbolOfWrongClass(symbol.as_str().to_string()));
            }
            if context.mutator.heap.canonical_symbol(symbol).is_some() {
                return Err(DecodeError::DuplicateSymbol(symbol.as_str().to_string()));
            }
            context
                .mutator
                .heap
                .record_canonical_symbol(symbol, *object);
        }
    }
    // Every record is in, so a future's computation is a closure now or
    // never: `future_state` reads an unresolved future with anything else
    // there as malformed.
    for computation in &pending_futures {
        match computation.as_object() {
            Some(closure) if context.mutator.heap.is_closure(closure) => {}
            _ => return Err(DecodeError::FutureRecord),
        }
    }
    // Every check for a future stays off a program's paths until one exists
    // (`Mutator::new_future`); a loaded one exists as surely as a made one.
    if !pending_futures.is_empty() {
        context.mutator.futures_made = true;
    }
    rebuild_applications(context, &objects, pending_classes, &pending_metaclasses)?;
    context.mutator.heap.stamp_dispatch_ids(&objects);
    for frame in &frames {
        let decoded = decode_frame(reader, context, &tables, symbols)?;
        context.mutator.heap.frame_replace(*frame, decoded);
    }
    check_class_references(&context.mutator.heap, &objects, &frames)?;
    let roots = decode_values(reader, context, &tables)?;
    Ok(DecodedGraph {
        roots,
        max_identity_hash,
        objects,
        frames,
    })
}

/// The inverse of `encode::encode_tag_named_hashes`: puts each recorded hash
/// back onto the object the bootstrap -- or `build_root_class`, or a
/// platform path's sends -- built, and answers the largest one, which `load`
/// advances the counter past together with the graph's own maximum. A 0, a
/// repeated object and `u32::MAX` are refused, each for the reason its
/// `DecodeError` gives.
pub fn decode_tag_named_hashes<'gc>(
    mutator: &mut Mutator<'gc>,
    reader: &mut Reader,
    modules: &ModuleClasses<'gc>,
    platform_paths: &[Value<'gc>],
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
            HASHED_MODULE_CLASS => read_module_class(reader, modules)?,
            HASHED_MODULE_LEVEL => read_module_level(reader, modules)?,
            HASHED_PLATFORM => match mutator.platform_value().as_object() {
                Some(platform) => platform,
                _ => unreachable!("the platform singleton is always an object"),
            },
            HASHED_PLATFORM_PATH => {
                let index = reader.read_u32()?;
                match platform_paths
                    .get(index as usize)
                    .and_then(|value| value.as_object())
                {
                    Some(object) => object,
                    _ => return Err(DecodeError::BadPlatformPath(index)),
                }
            }
            HASHED_METACLASS => {
                let class = read_prebuilt_class(mutator, reader, modules, platform_paths)?;
                mutator
                    .heap
                    .class_metaclass(class)
                    .ok_or(DecodeError::NotAClassReference)?
            }
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
        mutator.heap.object_set_identity_hash(target, hash);
        max = max.max(hash);
    }
    Ok(max)
}

/// The inverse of `encode::encode_class_record`: the mixin this class
/// applies, then its superclass, its enclosing object and its declaring
/// class, and -- for a level a `<:` clause added -- that clause.
///
/// The mixin id is range-checked here, against the decoded mixin table,
/// before it is used to index anything, and so are a level's applying row
/// and clause position: the clause is read off that row, so a level can only
/// ever run a clause message `verify_mixins` reconciled with its row's
/// factory.
///
/// A level whose clause named a builtin (`MIXIN_BUILTIN`) is its own shape:
/// its mixin, enclosing object and declaring class are the loading
/// bootstrap's builtin's, exactly what `nested::build_applied_levels` gives a
/// level it builds, so the record carries only the builtin, the superclass and
/// the clause. The three references resolve
/// to identities that already exist (every record was placeholder-allocated
/// before this pass started), so a forward reference and a cycle are both
/// ordinary; what cannot be settled yet is whether each really is a class
/// record, which `check_class_references` establishes afterwards.
fn decode_class_record<'gc>(
    reader: &mut Reader,
    context: &mut DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    placeholder_application: &Rc<crate::mixin::MixinApplication>,
) -> Result<(crate::types::ClassData<'gc>, Rc<crate::mixin::Mixin>), DecodeError> {
    let mixin = match reader.read_u8()? {
        MIXIN_ROW => {
            let id = reader.read_u32()?;
            context
                .mixins
                .get(id as usize)
                .cloned()
                .ok_or(DecodeError::BadMixinId(id))?
        }
        MIXIN_BUILTIN => {
            return decode_builtin_level(reader, context, tables, placeholder_application)
        }
        tag => {
            return Err(DecodeError::InvalidTag {
                what: "mixin reference",
                tag,
            })
        }
    };
    let superclass = decode_optional_class(reader, context, tables)?;
    let enclosing = if reader.read_boolean()? {
        match decode_value(reader, context, tables)?.as_object() {
            Some(object) => Some(object),
            _ => return Err(DecodeError::EnclosingNotAnObject),
        }
    } else {
        None
    };
    let declaring_class = decode_optional_class(reader, context, tables)?;
    let applied_clause = if reader.read_boolean()? {
        Some(decode_applied_clause(reader, context, None)?)
    } else {
        None
    };
    // A record only ever describes a user class: a builtin is a class *tag*,
    // resolved through the bootstrap, and never an object-table entry. Its
    // metaclass is its header, which `pair_metaclasses` holds to it once
    // every record is read.
    let mut data = crate::types::ClassData::new(
        placeholder_application.clone(),
        enclosing,
        declaring_class,
        superclass,
        None,
    );
    data.applied_clause = applied_clause.map(Box::new);
    Ok((data, mixin))
}

/// A `MIXIN_BUILTIN` class record, after its tag: the builtin whose mixin the
/// level applies, its superclass and the clause that added it. The builtin is
/// read as a program names one, so neither the VM mirror's class nor `Top`
/// can be applied.
fn decode_builtin_level<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    placeholder_application: &Rc<crate::mixin::MixinApplication>,
) -> Result<(crate::types::ClassData<'gc>, Rc<crate::mixin::Mixin>), DecodeError> {
    let kind = read_program_builtin_class(reader)?;
    let named = context.mutator.builtin_class(kind);
    let heap = &context.mutator.heap;
    let mixin = heap
        .class_application(named)
        .map(|application| application.mixin.clone())
        .ok_or(DecodeError::NotAClass)?;
    let superclass = decode_optional_class(reader, context, tables)?;
    let applied_clause = decode_applied_clause(reader, context, Some(kind))?;
    let mut data = crate::types::ClassData::new(
        placeholder_application.clone(),
        heap.class_enclosing(named),
        heap.class_declaring(named),
        superclass,
        None,
    );
    data.applied_clause = Some(Box::new(applied_clause));
    Ok((data, mixin))
}

/// The clause that added a level: the applying declaration's row id and the
/// clause's position in it, each checked before it indexes anything.
fn decode_applied_clause(
    reader: &mut Reader,
    context: &DecodeContext<'_, '_>,
    named_builtin: Option<BuiltinClass>,
) -> Result<crate::mixin::AppliedClause, DecodeError> {
    let row = reader.read_u32()?;
    let declaration = context
        .mixins
        .get(row as usize)
        .cloned()
        .ok_or(DecodeError::BadMixinId(row))?;
    let index = reader.read_u32()?;
    if index as usize >= declaration.mixin_applications.len() {
        return Err(DecodeError::BadMixinApplicationClause { row, index });
    }
    Ok(crate::mixin::AppliedClause {
        declaration,
        index: index as usize,
        named_builtin,
    })
}

// ---------------------------------------------------------------------------
// Reconciling the decoded graph with the code that runs in it.
//
// `decode_graph_section` reads frames and closures in isolation: it has no
// selector-to-body mapping, so it can check a closure record's copied values
// against the code it names (`CopiedCountMismatch`) and nothing more. Once
// the decoded mixins carry their code, every frame's body can be resolved --
// and is, because a frame's program counter, its register file and a
// closure's captured frame mean nothing except against that body.
//
// A register file the body overruns is no longer a memory-safety hole:
// `Heap::frame_local` answers `Nil` for an index a returned frame lacks and
// `frame_set_local` ignores the write, which is what keeps a crafted image
// from aborting the process. It is still refused here, because a frame
// narrower than its body is not something a run produces. An *empty*
// register file is, though, and is accepted: closure conversion leaves one
// behind for every returning activation no by-reference closure can reach
// ("Converted representation", "The divorce", in
// `docs/design/execution.md`).
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
        // The level names its clause, and the clause its body: the applying
        // class's mixin holds one message body per clause, so the class alone
        // would not say which ran.
        FrameKind::MixinApplicationMessage(level) => heap
            .class_applied_clause(*level)
            .and_then(|clause| clause.message_code),
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
///    same `arguments` count, and at least as many registers -- or be empty,
///    which is what a returning activation nothing reads by reference leaves
///    behind. (At least, not
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
    heap: &Heap<'gc>,
    graph: &DecodedGraph<'gc>,
    table: &DecodedCodeTable,
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
        if program_counter >= code.instruction_count() {
            return Err(DecodeError::ProgramCounterOutOfRange {
                program_counter,
                instructions: code.instruction_count(),
            });
        }
        // An empty register file is what a returning activation that was
        // never marked leaves behind (`Heap::pop_activation`): nothing
        // reaches its registers any more, so it keeps none. Every other
        // shape is still held to the body's.
        if argument_count + temporary_count != 0
            && (argument_count != code.argument_count as usize
                || argument_count + temporary_count < code.register_count as usize)
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
            verify_closure(heap, &closure, table, &parents, &index_of)?;
        }
    }
    Ok(())
}

fn verify_closure<'gc>(
    heap: &Heap<'gc>,
    closure: &ClosureData<'gc>,
    table: &DecodedCodeTable,
    parents: &[Option<usize>],
    index_of: &HashMap<usize, usize>,
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
