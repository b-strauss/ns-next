//! Canonical decoding, the inverse of `encode.rs`. A malformed byte stream
//! is a `DecodeError`, never a panic.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use ns_syntax::Span;

use crate::Symbol;
use crate::bytecode::leftovers::{self, LOOP_LIMIT, LeftoverRefusal, PENDING_LIMIT};
use crate::bytecode::{
    ARGUMENT_LIMIT, BodyLimit, CallSite, CodeBuilder, CodeKind, CompiledCode, CreatedBlock,
    Emitted, HEADER_REGISTERS, INSTRUCTION_LIMIT, Instruction, IntegerOperation, Literal,
    jump_target, register_written,
};
use crate::heap::{
    FrameRecord, Heap, IDENTITY_HASH_LIMIT, NewClass, PlaceholderFormat, record_size,
};
use crate::interpreter::instantiate;
use crate::numeric::Rational;
use crate::types::{
    BuiltinClass, ClassReference, FrameKind, FrameReference, ObjectReference, Value,
    fits_small_integer,
};
use crate::{ArbitraryInteger, Mutator};

use super::opcodes::*;
use super::shipped_names::{NamePosition, ResolvedName, ShippedKind, ShippedName};
use super::{mixins, symbols};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    UnexpectedEndOfFile,
    BadMagic,
    VersionMismatch {
        found: u32,
        expected: u32,
    },
    /// The header's fingerprint is not this binary's: the image names the
    /// shipped world by declaration, and another world's declarations are
    /// not this one's.
    ShippedWorldMismatch {
        found: u64,
        expected: u64,
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
    /// A field the format types as a frame reference -- a frame's sender or
    /// home, a closure's captured frame -- names an object id whose record
    /// is not a frame record (`OBJECT_FRAME`).
    NotAFrameReference(u32),
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
    /// A frame record (`OBJECT_FRAME`) whose header is not the builtin
    /// `Activation`. Every frame object is an instance of that class, so
    /// this describes an activation answering some other class's methods,
    /// which no live world has. The other half -- `Activation`'s header on
    /// another record -- is refused as `Activation` named anywhere else is.
    FrameRecordUnderAnotherClass,
    /// A record under the builtin `FutureImplementation` header that is not an
    /// unresolved future's: an instance record of exactly two fields, a closure
    /// and the state 0. The encoder writes a resolved future as its value and a
    /// resolving one as unresolved (`docs/design/image-format.md`, "Futures in
    /// a graph"), so any other record describes a future no encoder wrote --
    /// one `Mutator::future_state` reads as malformed, or a resolved one
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
    /// A module record names its module as no project could: a path that
    /// is not relative to the project or no `core:` file name, an id that
    /// is no id or does not end in its root class's name, an id in the
    /// namespace `newspeak.` outside the binary's modules or one outside it
    /// for them, or a version that is no semver (`docs/design/modules.md`,
    /// decision 10). `reason` is the rule's own message.
    ModuleIdentity {
        module: u32,
        reason: String,
    },
    /// Two module records no request could pick between: of one id and one
    /// version, or of one id where either has none. The message names both
    /// by their paths.
    IndistinctModules(String),
    /// A text of the text table whose parts do not describe a declaration:
    /// holes out of order, overlapping or outside its range, or a cut text
    /// whose length is not the range's less the holes'
    /// (`DeclarationText::from_parts`). Slicing it could not answer the
    /// text a span names.
    MalformedText {
        text: u32,
    },
    /// A mixin-table row names a text past the text table.
    TextIndexOutOfRange {
        row: u32,
        index: u32,
        texts: usize,
    },
    /// A mixin-table row names a text with another number of holes than
    /// the declaration has nested rows: its module's text could not be
    /// assembled from its declarations.
    TextHolesMismatch {
        row: u32,
        holes: usize,
        nested: usize,
    },
    /// Two instance-side rows name one text. A text belongs to one
    /// declaration; shared, it would be assembled into the module's text
    /// once per row, and a short image could make the load build an
    /// arbitrarily large one.
    TextSharedByRows {
        text: u32,
        first: u32,
        second: u32,
    },
    /// A class-side row names another text than its instance side's, which
    /// is the one text of their declaration.
    ClassSideTextMismatch {
        row: u32,
        index: u32,
        instance_side: u32,
    },
    /// A text of the text table no row names: bytes the image carries for
    /// no declaration.
    UnnamedText {
        text: u32,
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
    /// A code record's parameters, the header's numbers and its copied
    /// values together do not fit its register file. `push_step` sizes an
    /// activation's registers above its header as `register_count -
    /// argument_count - HEADER_REGISTERS`, so this would leave
    /// `fill_copied_values` writing past the registers the body has.
    CopiedCountOutOfRange {
        argument_count: u16,
        copied_count: u16,
        register_count: u16,
    },
    /// A class record names a `<:` clause its applying row does not declare:
    /// `index` is past the row's clauses. A level's clause is where its
    /// selector and compiled message come from, so there is nothing a level
    /// with no clause could initialize by.
    BadMixinApplicationClause {
        row: u32,
        index: u32,
    },
    /// A class record's `<:` clause index at or past
    /// `CLAUSE_INDEX_LIMIT`. The resolver refuses such a source, so refusing
    /// it is the only answer that does not truncate it into another clause.
    ClauseIndexTooLarge {
        row: u32,
        index: u32,
    },
    /// A mixin-table row of more `<:` clauses than `CLAUSE_INDEX_LIMIT`,
    /// refused where its count is read: the resolver refuses such a source
    /// (`docs/design/compiler.md`, "Limits").
    ClauseCountTooLarge {
        count: usize,
    },
    /// A body's arity, a method's or factory's, a closure's parameter count,
    /// or a send's or clause message's argument count past
    /// `ARGUMENT_LIMIT`, refused where it is read: the compiler makes such a
    /// body one that raises, and the encoder refuses to write it
    /// (`docs/design/compiler.md`, "Limits"). `what` names the record.
    ArgumentCountTooLarge {
        what: &'static str,
        count: usize,
    },
    /// A body of more instructions than `INSTRUCTION_LIMIT`, refused where
    /// its count is read, before any instruction is: no compiled body is
    /// longer (`docs/design/compiler.md`, "Limits").
    InstructionCountTooLarge {
        count: u32,
    },
    /// A mixin-table row declares `<:` clauses but no superclass clause. The
    /// grammar admits `<:` only after a superclass clause, and it is the
    /// superclass clause's body that evaluates every clause's receiver
    /// (`interpreter::nested::superclass_clause_expression`), so such a row
    /// describes a declaration no source has and whose levels nothing builds.
    MixinApplicationWithoutSuperclass,
    /// A mixin-table row says it has no body (`Mixin::has_body`) but
    /// declares no `<:` clause: only a mixin application may end in a dot
    /// (§6's `mixinAppSuffix`), and its class object is the level its last
    /// clause adds, which such a row has none of.
    BodyLessWithoutClauses,
    /// A mixin-table row says it has no body yet declares members of one:
    /// slots, `initExpr`s, methods, a class side or nested declarations.
    /// Nothing could run or reach them,
    /// since no class applies a body-less declaration's mixin.
    BodyLessWithMembers,
    /// A class record applies a body-less row's mixin. A body-less
    /// declaration's class object applies its last clause's mixin (decision
    /// 8 of `new` as compiled code), so no live class has one, and its full
    /// body counts its levels from a class object of another mixin.
    BodyLessMixinApplied,
    /// A class record's body tag disagrees with its clause: an
    /// `OBJECT_BODY_LESS_CLASS` record is a body-less declaration's class
    /// object, the level its last clause added, and carries that
    /// declaration's scope; an `OBJECT_CLASS` record is any other class
    /// object and carries none. A record written with the other's tag would
    /// give a level a scope it has not got, or take one away from the
    /// declaration's own code.
    BodyLessClassRecordMismatch,
    /// The classes the `<:` clauses of levels named form a cycle: each
    /// level's metaclass is rebuilt from the class its clause named, so a
    /// cycle names no class side at all. A live clause names a class that
    /// existed before its level did.
    NamedClassCycle,
    /// A module has no tree in the image's mixin table, or the image carries
    /// a class it has no application for, so there is no declaration to
    /// build a class on.
    NotAClass,
    /// The record the module section names as a module's root class is no
    /// class of that module's root declaration as its outermost level: a
    /// record of another kind, a metaclass, a level, a class of another
    /// declaration, or one with an enclosing object or a declaring class. A
    /// module's root is the class a host instantiates, so the record must be
    /// the one its own root row describes.
    ModuleRootNotItsRecord {
        module: u32,
    },
    /// A module's root class record inherits what the root rule does not
    /// build (`resolver::root_clause_defect`): its superclass must be
    /// `Kernel Object` or `Kernel Value`, or, for a root with its one
    /// permitted `<:` clause, the level that clause added, applying `Kernel
    /// Value` over `Kernel Object`. The encoder writes no other root, so a
    /// crafted artifact cannot bring back one the resolver refuses.
    RootRule {
        module: u32,
    },
    /// A selector the image names has no declaration to bind to: a
    /// mixin-table row's code list naming a selector its own method
    /// dictionary lacks.
    UnknownSelector(String),
    /// A mixin-table row declares slots or `initExpr`s but carries no
    /// compiled own part of its initializer. A `<:` clause applying such a
    /// mixin runs that code on every instantiation, and nothing would be
    /// there to run -- so the load refuses it rather than let the level
    /// initialize nothing.
    MissingInitializer,
    /// A mixin-table row carries no full instance initializer, which every
    /// declaration compiles: its class's factory runs it on every
    /// instantiation.
    MissingFullInitializer,
    /// A mixin-table row writes `<:` clauses but carries no standalone
    /// instance initializer, which a level of its mixin reached from
    /// outside runs.
    MissingStandaloneInitializer,
    /// A mixin-table row writes no `<:` clause, or no body, yet carries a
    /// standalone instance initializer, which no source compiles for such a
    /// declaration.
    UnexpectedStandaloneInitializer,
    /// A mixin-table row declares a superclass clause the image carries no
    /// compiled clause for. The nested-class accessor runs that code on the
    /// first access to the declaration, and panics on the invariant when
    /// the mixin carries none -- the same reason `MissingInitializer` is a
    /// refusal rather than a deferred crash.
    MissingSuperclassClause,
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
    /// different number of arguments than its own declaration does. A send
    /// (or a factory) is checked against the declaration, and its arguments
    /// open the activation's register window, followed by the body's
    /// `register_count - argument_count` temporaries; a disagreement leaves
    /// the window shorter than the registers the body addresses, so a read
    /// runs past its end, or longer, so every temporary is shifted.
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
    /// A send's argument slots running past its own body's register file:
    /// the slots `argument_base .. argument_base + argument_count`, whose
    /// last is out of range even when the base is not.
    ArgumentSlotsOutOfRange {
        argument_base: u16,
        argument_count: usize,
        register_count: u16,
    },
    /// An outer-local operand outside the register file of the body it
    /// addresses -- the one `depth` closure hops out.
    OuterLocalOutOfRange {
        depth: u16,
        index: u16,
        register_count: u16,
    },
    /// A register operand -- of the body's own frame at depth 0, an outer
    /// local of the body `depth` closure hops out otherwise -- that names
    /// one of the frame header's numbers, `argument_count ..
    /// argument_count + HEADER_REGISTERS`: the register would be a word of
    /// a live frame's header, its caller or its register count among them.
    HeaderRegister {
        depth: u16,
        register: u16,
        argument_count: u16,
    },
    /// A send's argument slots starting below the first register above its
    /// body's header: its callee is laid on them, its header where they
    /// end, so slots among the sender's own arguments or header numbers
    /// would put that header over the sender's.
    ArgumentSlotsBelowTheHeader {
        argument_base: u16,
        argument_count: u16,
    },
    /// A send's argument slots starting below its body's `local_count`, the
    /// first scratch register: the callee's header would lie among the
    /// registers a block reaches by reference.
    ArgumentSlotsBelowTheScratch {
        argument_base: u16,
        local_count: u16,
    },
    /// A send one of whose sources is another of its own argument slots:
    /// the send writes each source into its slot in turn, so what that
    /// source answers would depend on the order of the writes.
    SourceIsAnotherSlot {
        argument_base: u16,
        position: usize,
        source: u16,
    },
    /// An instruction writing an argument register -- of its own frame at
    /// depth 0, of the body `depth` closure hops out otherwise. Parameters
    /// are immutable (§3.6), and an argument register is the sender's
    /// argument slot, whose value the sender may read after the callee
    /// returns.
    ArgumentRegisterWritten {
        depth: u16,
        register: u16,
        argument_count: u16,
    },
    /// An outer local at or past the `local_count` of the body `depth`
    /// closure hops out: a scratch register of that frame, where a callee
    /// it is sending to lies, its header among them.
    OuterLocalInScratch {
        depth: u16,
        index: u16,
        local_count: u16,
    },
    /// A read of `register` at `instruction` where it may still hold what
    /// a callee left there -- above the slots of a send before it, where
    /// the callee was laid -- or the word the push left in a scratch
    /// register: no compiled body reads one before writing it, and a callee
    /// may have left the VM mirror there (`bytecode::leftovers`;
    /// `docs/design/compiler.md`, "What a callee leaves").
    LeftoverRead {
        instruction: u32,
        register: u16,
    },
    /// A body the leftover analysis refuses at `instruction` before it
    /// allocates for it: more jump targets pending or loops open than any
    /// compiled body has, crossing loops, or a jump into a loop past its
    /// head (`bytecode::leftovers`; `docs/design/compiler.md`, "What a
    /// callee leaves").
    VerificationPastTheBound {
        instruction: u32,
    },
    /// A `local_count` past its body's register file.
    LocalCountOutOfRange {
        local_count: u16,
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
    /// A body-less declaration's full initializer named anywhere but as
    /// that declaration's full initializer: as another body of its own
    /// row, or as a body of another row. The body is its declaration's own
    /// code (`CompiledCode::is_declaration_code`), whose lexical sends and
    /// `outer` start from the declaration's scope, so run as any other
    /// body it would start them there too.
    SharedDeclarationCode,
    /// A decoded frame's register file does not match the body that runs in
    /// it -- its arguments and then its registers above the header
    /// (`CompiledCode::frame_register_count`) -- so a register index the
    /// body considers in range would index past the frame's fields.
    /// `expected_locals` is how many registers the body has above its
    /// header, `register_count` its whole numbering, the header's included.
    FrameRegisterFile {
        arguments: usize,
        locals: usize,
        argument_count: u16,
        expected_locals: usize,
        register_count: u16,
    },
    /// A decoded closure's captured frame is not an activation of the body
    /// its own body is lexically nested in -- the link every
    /// `LoadOuterLocal`/`StoreOuterLocal` bound rests on.
    CapturedFrameMismatch,
    /// A decoded frame's body is not one its kind runs: a method frame's
    /// not a method body of its method's argument count, a closure frame's
    /// not its closure's own code, a clause frame's not a body of that
    /// clause's kind, or a body named for a top-level frame, or none for
    /// any other; or a body-less declaration's own code in a frame that
    /// does not run it at that declaration's class object, or other code
    /// in a frame that would (`Heap::frame_runs_declaration_code`). The
    /// program counter and the register file mean nothing against a body
    /// the frame never ran.
    FrameCodeMismatch,
    /// A frame record is not marked uncontinuable. An image is written only
    /// when no activation runs, so every frame in a genuine one has
    /// returned; a frame that could be resumed would continue from
    /// registers and a program counter the image's author chose.
    ContinuableFrame,
    /// A frame's `home` is not populated exactly when its kind is a
    /// closure's: a closure activation's home is the activation a `^` in its
    /// body returns from, and no other activation has one.
    FrameHomeMismatch,
    /// A frame record names its body by declaration (`FRAME_BODY_DECLARED`)
    /// where the class it resolves through applies no shipped mixin, or
    /// whose kind is a closure's or a top-level one: only a shipped body,
    /// which the code table never carries, is named so, and every other has
    /// one spelling, its code id.
    DeclaredBodyNotShipped,
    /// An identity hash the header word cannot hold: it is 22 bits wide.
    /// Refusing the image is the only answer that does not alias the hash
    /// to another one.
    IdentityHashTooWide(u32),
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
    /// An `OBJECT_STRING` record's class tag names the builtin `Symbol`
    /// class. A Symbol object is a String of that class, so the record
    /// would be a second Symbol of its text, outside the canonical table,
    /// and two Symbols of one text would not be identical (§5.1.6).
    StringOfSymbolClass,
    /// An `OBJECT_BYTE_ARRAY` record under a header whose class is not the
    /// builtin `ByteArray`, or a record of any other body, `body`, under
    /// `ByteArray`'s. A record carries exactly its class's layout: the
    /// first would answer that class's methods over bytes they do not
    /// read, the second `ByteArray`'s leaves over an object holding none.
    ByteArrayRecordMismatch {
        body: u8,
    },
    /// An `OBJECT_WEAK_ARRAY` record under a header whose class is not the
    /// builtin `WeakArray`, or a record of a body, `body`, other than
    /// `OBJECT_WEAK_ARRAY` and `OBJECT_INSTANCE` under `WeakArray`'s. The
    /// first would be an object of that class holding weakly what its
    /// methods take for strong slots; the second would answer `WeakArray`'s
    /// leaves over an object of another shape, which no program makes.
    /// `WeakArray`'s header carries an `OBJECT_INSTANCE` too, the slotless
    /// instance `withoutSlots` makes (`WeakArrayInstanceFieldCount`).
    WeakArrayRecordMismatch {
        body: u8,
    },
    /// An `OBJECT_INSTANCE` record under `WeakArray`'s header of `found`
    /// fields where the class declares `expected` slots: the instance
    /// `withoutSlots` makes has exactly the class's slots, none, and a
    /// record of more would be an object no program makes.
    WeakArrayInstanceFieldCount {
        expected: usize,
        found: usize,
    },
    /// An `OBJECT_EPHEMERON` record under a header whose class is not the
    /// builtin `Ephemeron`, or a record of any other body, `body`, under
    /// `Ephemeron`'s. The first would have the collectors read that class's
    /// first slot as a key; the second would hold strongly what an
    /// `Ephemeron`'s methods hold through its key, and no program makes one.
    EphemeronRecordMismatch {
        body: u8,
    },
    /// An `OBJECT_EPHEMERON` record of `found` fields where `Ephemeron`
    /// declares `expected`: its methods read the three slots it declares,
    /// the key first.
    EphemeronFieldCount {
        expected: usize,
        found: usize,
    },
    /// An `OBJECT_MIXIN` record under a header whose class is not the
    /// kernel-private `Mixin`, or a record of any other body, `body`, under
    /// `Mixin`'s. The first would make an object of that class stand for a
    /// mixin; the second an object of `Mixin` standing for whatever mixin
    /// its field's id named, outside the table that keeps one object per
    /// mixin, and no program makes one.
    MixinRecordMismatch {
        body: u8,
    },
    /// An `OBJECT_MIXIN` record's side byte is neither `MIXIN_SIDE_INSTANCE`
    /// nor `MIXIN_SIDE_CLASS`.
    BadMixinSide(u8),
    /// An `OBJECT_MIXIN` record names a class side's own row. A class side
    /// is named through its instance side, with `MIXIN_SIDE_CLASS`, so each
    /// mixin has one spelling on the wire, and no record names the class
    /// side of a class side, which no class has. A second record for one
    /// mixin is refused by the mixin's id either way (`DuplicateMixin`).
    MixinRecordNamesAClassSide,
    /// An `OBJECT_MIXIN` record's shipped name, shown here, names a module
    /// instance, a metaclass, a level or a level's metaclass: only a class's
    /// declaration is a mixin a record names.
    MixinRecordNamesNoClass(String),
    /// Two `OBJECT_MIXIN` records for one mixin, or one for a mixin the heap
    /// has reified already, named here: a reified mixin is the one object
    /// of its mixin, as a symbol is of its text (`DuplicateSymbol`), so a
    /// second is refused, not merged.
    DuplicateMixin(String),
    /// Bytes remain after the last section. Refused rather than ignored:
    /// silently accepting a tail means a padded image loads as something its
    /// author did not write.
    TrailingBytes {
        count: usize,
    },
    /// The shipped-name section lists more names than
    /// `shipped_names::MAX_SHIPPED_NAMES`, which the count is held to before
    /// anything is reserved for it.
    TooManyShippedNames {
        count: usize,
    },
    /// A shipped name the loading binary's shipped world has no object for:
    /// an unknown module, an unknown declaration, or a level past its
    /// declaration's clauses.
    UnknownShippedName(String),
    /// The shipped-name section lists one name twice. The encoder lists each
    /// name once, so a second entry would give one object two indices.
    DuplicateShippedName(String),
    /// A shipped-name reference whose index is past the image's section.
    BadShippedName(u32),
    /// A shipped name standing where the rule of what may be named where
    /// forbids it (`docs/design/image-format.md`, "Shipped names"): `Top` or
    /// `Activation` as a value, a module root as a
    /// record's class, a module instance outside a frame. `position` is
    /// `NamePosition::describe`'s text.
    ShippedNameNotAllowed {
        name: String,
        position: &'static str,
    },
    /// A started image names a shipped object of a module other than the
    /// kernel, or a module's instance, the kernel's included. Before
    /// `main:args:` a program reaches nothing of the other modules, and no
    /// program reaches a module's instance, so no encoder writes such an
    /// image.
    ShippedNameInStartedImage(String),
    /// An `OBJECT_INSTANCE` record of a shipped class that is no builtin
    /// carries a field count other than that class's layout. The layout
    /// comes from the loading binary, not from the image, so a mismatch
    /// would leave the class's code reading fields the record does not
    /// have, or not reading ones it does.
    ShippedInstanceFieldCount {
        expected: usize,
        found: usize,
    },
    /// A record under the header of a shipped class that is no builtin with
    /// a body other than `OBJECT_INSTANCE`. Such a class's instances are
    /// instances with slots, laid out by the loading binary; an array,
    /// string, closure, class or frame record under its header would be an
    /// object its code reads as something it is not.
    ShippedInstanceBody {
        body: u8,
    },
    /// An instance of a program class -- one that is no builtin and whose
    /// chain does not reach `Behavior` -- carries a field count other than
    /// its class's application's slot count. Every such instance is
    /// allocated with exactly that count, and an install that changes a
    /// layout migrates every instance to the new one, so no encoder writes
    /// another; a customized copy reads its own slots at fixed fields with
    /// no check (`jit::optimizing::graph::Node::OwnSlotLoad`), past the end
    /// of an object that has fewer.
    ProgramInstanceFieldCount {
        expected: usize,
        found: usize,
    },
    /// An object of a program class that is not an instance record: an
    /// array, a string, a closure or a class under the header of a class
    /// whose instances are all ordinary instances with slots, which its
    /// code -- a customized copy's unchecked slot reads above all -- would
    /// read as what it is not.
    ProgramInstanceBody,
    /// A `LargeInteger`, as a value or a literal, in an encoding the encoder
    /// never writes: one that fits `i64` (which travels as an `i64`), a zero
    /// top digit, or a negative zero. Accepting one would give an integer a
    /// second encoding, and a decoder that trusted the tag rather than the
    /// range would load a second representation -- a `LargeInteger` object
    /// equal to a `SmallInteger`, which every arithmetic fast path would
    /// treat as a different value.
    NonCanonicalInteger,
    /// A `VALUE_FRACTION` not in the normal form the encoder writes and the
    /// runtime keeps every fraction in: a denominator of 1 or below, or a
    /// factor its two parts share. `=` and `hash` compare a fraction's
    /// parts, so a second form of one quotient would be a different value,
    /// and a denominator of 1 an integer that answers as no integer does.
    NonNormalFraction,
    /// A record under the builtin `Fraction` header other than an instance
    /// record of no field, `Fraction new`'s. A fraction travels only as a
    /// `VALUE_FRACTION`, whose normal form the decoder checks; a record of
    /// fields under that header would make an object every `Fraction` leaf
    /// reads as a fraction, its fields unchecked.
    FractionRecord,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::UnexpectedEndOfFile => write!(f, "unexpected end of image"),
            DecodeError::BadMagic => write!(f, "not an ns-next image (bad magic)"),
            DecodeError::VersionMismatch { found, expected } => {
                write!(
                    f,
                    "image format version {found} is not supported (this build reads version {expected})"
                )
            }
            DecodeError::ShippedWorldMismatch { found, expected } => write!(
                f,
                "the image was written by another shipped world (fingerprint {found:016x}, this binary's {expected:016x})"
            ),
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
            DecodeError::ProgramCounterOutOfRange {
                program_counter,
                instructions,
            } => write!(
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
            DecodeError::NotAFrameReference(id) => {
                write!(
                    f,
                    "object {id} is named as a frame but is not a frame record"
                )
            }
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
            DecodeError::FrameRecordUnderAnotherClass => write!(
                f,
                "a frame record in the image names a class other than Activation"
            ),
            DecodeError::FutureRecord => write!(
                f,
                "an object in the image is a FutureImplementation without an unresolved \
                 future's record"
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
                "the image's mixin table is not a sequence of declaration trees: row {row} is \
                 named as a nested declaration or class side out of declaration order, or twice"
            ),
            DecodeError::ModuleCountMismatch { declared, trees } => write!(
                f,
                "the image declares {declared} module(s) but its mixin table holds {trees} tree(s)"
            ),
            DecodeError::ModuleIdentity { module, reason } => {
                write!(f, "module {module} of the image: {reason}")
            }
            DecodeError::IndistinctModules(reason) => write!(f, "the image's modules: {reason}"),
            DecodeError::MalformedText { text } => {
                write!(
                    f,
                    "text {text} of the image does not describe a declaration"
                )
            }
            DecodeError::TextIndexOutOfRange { row, index, texts } => write!(
                f,
                "mixin-table row {row} names text {index}, and the image carries {texts}"
            ),
            DecodeError::TextHolesMismatch { row, holes, nested } => write!(
                f,
                "mixin-table row {row}'s text has {holes} nested declarations, and the row {nested}"
            ),
            DecodeError::TextSharedByRows {
                text,
                first,
                second,
            } => write!(
                f,
                "text {text} of the image is named by mixin-table rows {first} and {second}, \
                 and a text belongs to one declaration"
            ),
            DecodeError::ClassSideTextMismatch {
                row,
                index,
                instance_side,
            } => write!(
                f,
                "mixin-table row {row}, a class side, names text {index}, and not the text of \
                 its instance side, row {instance_side}"
            ),
            DecodeError::UnnamedText { text } => {
                write!(f, "text {text} of the image is named by no mixin-table row")
            }
            DecodeError::DuplicateSelector(selector) => write!(
                f,
                "a mixin-table row declares member `{selector}` more than once"
            ),
            DecodeError::LazySlotWithoutGetter(slot) => write!(
                f,
                "a mixin-table row declares the lazy slot `{slot}` without its getter method"
            ),
            DecodeError::ClosureArityMismatch {
                parameters,
                argument_count,
            } => write!(
                f,
                "a `MakeClosure` names {parameters} parameters but the block body it creates takes {argument_count}"
            ),
            DecodeError::CopiedCountMismatch {
                copied,
                copied_count,
            } => write!(
                f,
                "the image supplies {copied} copied values for a block that is handed {copied_count}"
            ),
            DecodeError::CopiedCountOutOfRange {
                argument_count,
                copied_count,
                register_count,
            } => write!(
                f,
                "a body in the image takes {argument_count} arguments and {copied_count} copied \
                 values, which do not fit its {register_count} registers"
            ),
            DecodeError::BadMixinApplicationClause { row, index } => write!(
                f,
                "the image names mixin application clause {index} of mixin row {row}, which the \
                 row does not declare in that place"
            ),
            DecodeError::ClauseIndexTooLarge { row, index } => write!(
                f,
                "mixin application clause index {index} of row {row} is past the limit of {} clauses",
                crate::mixin::CLAUSE_INDEX_LIMIT
            ),
            DecodeError::ClauseCountTooLarge { count } => write!(
                f,
                "a mixin-table row applies {count} mixins, more than the limit of {}",
                crate::mixin::CLAUSE_INDEX_LIMIT
            ),
            DecodeError::ArgumentCountTooLarge { what, count } => write!(
                f,
                "a {what} in the image takes {count} arguments, more than the limit of {ARGUMENT_LIMIT}"
            ),
            DecodeError::InstructionCountTooLarge { count } => write!(
                f,
                "a body in the image holds {count} instructions, more than the limit of {INSTRUCTION_LIMIT}"
            ),
            DecodeError::MixinApplicationWithoutSuperclass => write!(
                f,
                "a mixin-table row applies mixins but declares no superclass clause to apply them to"
            ),
            DecodeError::NotAClass => {
                write!(f, "the image's mixin table describes no root class")
            }
            DecodeError::ModuleRootNotItsRecord { module } => write!(
                f,
                "the image names a record as the root class of its module {module} that is no \
                 class of that module's root declaration"
            ),
            DecodeError::RootRule { module } => write!(
                f,
                "the root class of the image's module {module} inherits neither Object nor \
                 Value, nor Value applied over Object, as a module's root class must"
            ),
            DecodeError::UnknownSelector(selector) => write!(
                f,
                "the image names method `{selector}`, which the declaration it belongs to does not define"
            ),
            DecodeError::MissingInitializer => write!(
                f,
                "a class in the image needs an instance initializer, which the image has no compiled code for"
            ),
            DecodeError::MissingFullInitializer => write!(
                f,
                "a class in the image has no compiled code for its instance initializer"
            ),
            DecodeError::MissingStandaloneInitializer => write!(
                f,
                "a class in the image applies mixins, and the image has no compiled code for its standalone instance initializer"
            ),
            DecodeError::UnexpectedStandaloneInitializer => write!(
                f,
                "a class in the image applies no mixin or has no body, yet the image carries a standalone instance initializer for it"
            ),
            DecodeError::MissingSuperclassClause => write!(
                f,
                "the image declares a superclass clause it has no compiled code for"
            ),
            DecodeError::BodyLessWithoutClauses => {
                write!(f, "a class in the image has no body and applies no mixin")
            }
            DecodeError::BodyLessWithMembers => write!(
                f,
                "a class in the image has no body, yet the image gives it members"
            ),
            DecodeError::BodyLessMixinApplied => write!(
                f,
                "a class object in the image applies the mixin of a class without a body"
            ),
            DecodeError::BodyLessClassRecordMismatch => write!(
                f,
                "a class record in the image is written as the class of a declaration \
                 without a body, or as any other class, and its mixin application clause \
                 says otherwise"
            ),
            DecodeError::NamedClassCycle => write!(
                f,
                "the classes the mixin application clauses in the image name form a cycle"
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
            DecodeError::ArgumentSlotsOutOfRange {
                argument_base,
                argument_count,
                register_count,
            } => write!(
                f,
                "{argument_count} argument slot(s) from r{argument_base} run past a body with {register_count} register(s)"
            ),
            DecodeError::OuterLocalOutOfRange {
                depth,
                index,
                register_count,
            } => write!(
                f,
                "outer local r{index} at depth {depth} is outside the enclosing body's {register_count} register(s)"
            ),
            DecodeError::HeaderRegister {
                depth,
                register,
                argument_count,
            } => write!(
                f,
                "register r{register} at depth {depth} names the frame header of a body taking {argument_count} argument(s)"
            ),
            DecodeError::ArgumentSlotsBelowTheHeader {
                argument_base,
                argument_count,
            } => write!(
                f,
                "argument slots from r{argument_base} lie below the frame header's end in a body taking {argument_count} argument(s)"
            ),
            DecodeError::ArgumentSlotsBelowTheScratch {
                argument_base,
                local_count,
            } => write!(
                f,
                "argument slots from r{argument_base} lie below the first scratch register r{local_count}"
            ),
            DecodeError::SourceIsAnotherSlot {
                argument_base,
                position,
                source,
            } => write!(
                f,
                "the source of argument {position} of a send with slots from r{argument_base} is r{source}, another of its slots"
            ),
            DecodeError::ArgumentRegisterWritten {
                depth,
                register,
                argument_count,
            } => write!(
                f,
                "register r{register} at depth {depth} is written, an argument of a body taking {argument_count} argument(s)"
            ),
            DecodeError::OuterLocalInScratch {
                depth,
                index,
                local_count,
            } => write!(
                f,
                "outer local r{index} at depth {depth} is a scratch register of the enclosing body, whose first is r{local_count}"
            ),
            DecodeError::LeftoverRead {
                instruction,
                register,
            } => write!(
                f,
                "register r{register} is read at instruction {instruction} before the body writes it: it may hold what a callee or the push left there"
            ),
            DecodeError::VerificationPastTheBound { instruction } => write!(
                f,
                "a body in the image passes the verification's bound at instruction {instruction}: no compiled body keeps more than {PENDING_LIMIT} jump targets pending or {LOOP_LIMIT} loops open, crosses two loops or jumps into a loop past its head"
            ),
            DecodeError::LocalCountOutOfRange {
                local_count,
                register_count,
            } => write!(
                f,
                "a local count of {local_count} is past a body with {register_count} register(s)"
            ),
            DecodeError::JumpTargetOutOfRange {
                target,
                instructions,
            } => write!(
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
            DecodeError::SharedDeclarationCode => write!(
                f,
                "a body-less declaration's instance initializer is named as another body too"
            ),
            DecodeError::FrameRegisterFile {
                arguments,
                locals,
                argument_count,
                expected_locals,
                register_count,
            } => write!(
                f,
                "a decoded frame holds {arguments} argument(s) and {locals} local(s), but the code that runs in it takes {argument_count} argument(s) and {expected_locals} local(s) beside its header in {register_count} register(s)"
            ),
            DecodeError::CapturedFrameMismatch => write!(
                f,
                "a decoded closure's captured frame does not run the body its own body is nested in"
            ),
            DecodeError::FrameCodeMismatch => {
                write!(f, "a frame's body is not one its kind runs")
            }
            DecodeError::ContinuableFrame => write!(
                f,
                "a frame is not marked uncontinuable, but an image only carries frames that have returned"
            ),
            DecodeError::FrameHomeMismatch => write!(
                f,
                "a frame has a home unless it is a closure's activation, or lacks one when it is"
            ),
            DecodeError::DeclaredBodyNotShipped => write!(
                f,
                "a frame names its body by declaration, which only a frame of shipped code may"
            ),
            DecodeError::IdentityHashTooWide(hash) => write!(
                f,
                "identity hash {hash} does not fit the header word's 22 bits"
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
                write!(
                    f,
                    "symbol #{text} is recorded with a class other than Symbol"
                )
            }
            DecodeError::StringOfSymbolClass => f.write_str(
                "a string record has the class Symbol, which only a symbol record may have",
            ),
            DecodeError::ByteArrayRecordMismatch { body } if *body == OBJECT_BYTE_ARRAY => {
                f.write_str("a byte-array record has a class other than ByteArray")
            }
            DecodeError::ByteArrayRecordMismatch { body } => write!(
                f,
                "a record of the class ByteArray has object body {body}, but a ByteArray is a \
                 byte-array record"
            ),
            DecodeError::WeakArrayRecordMismatch { body } if *body == OBJECT_WEAK_ARRAY => {
                f.write_str("a weak-array record has a class other than WeakArray")
            }
            DecodeError::WeakArrayRecordMismatch { body } => write!(
                f,
                "a record of the class WeakArray has object body {body}, but a WeakArray is \
                 a weak-array record or a slotless instance record"
            ),
            DecodeError::WeakArrayInstanceFieldCount { expected, found } => write!(
                f,
                "an instance record of the class WeakArray has {found} fields, but WeakArray \
                 declares {expected} slots"
            ),
            DecodeError::EphemeronRecordMismatch { body } if *body == OBJECT_EPHEMERON => {
                f.write_str("an ephemeron record has a class other than Ephemeron")
            }
            DecodeError::EphemeronRecordMismatch { body } => write!(
                f,
                "a record of the class Ephemeron has object body {body}, but an Ephemeron is \
                 an ephemeron record"
            ),
            DecodeError::EphemeronFieldCount { expected, found } => write!(
                f,
                "an ephemeron record has {found} fields, but an Ephemeron has {expected}"
            ),
            DecodeError::MixinRecordMismatch { body } if *body == OBJECT_MIXIN => {
                f.write_str("a mixin record has a class other than Mixin")
            }
            DecodeError::MixinRecordMismatch { body } => write!(
                f,
                "a record of the class Mixin has object body {body}, but a Mixin is a mixin \
                 record"
            ),
            DecodeError::BadMixinSide(side) => write!(
                f,
                "a mixin record has side {side}, but a side is 0, the instance side, or 1, the \
                 class side"
            ),
            DecodeError::MixinRecordNamesAClassSide => f.write_str(
                "a mixin record names a class side's row; a class side is named by its instance \
                 side's, with side 1",
            ),
            DecodeError::MixinRecordNamesNoClass(name) => write!(
                f,
                "a mixin record names {name}, which is no class's shipped name"
            ),
            DecodeError::DuplicateMixin(name) => write!(f, "the mixin {name} is recorded twice"),
            DecodeError::TrailingBytes { count } => {
                write!(f, "{count} trailing byte(s) after the last section")
            }
            DecodeError::TooManyShippedNames { count } => write!(
                f,
                "the image lists {count} shipped names, more than the {} a loader resolves",
                super::shipped_names::MAX_SHIPPED_NAMES
            ),
            DecodeError::UnknownShippedName(name) => write!(
                f,
                "the image names {name}, which this binary's shipped world does not have"
            ),
            DecodeError::DuplicateShippedName(name) => {
                write!(f, "the image lists the shipped name {name} twice")
            }
            DecodeError::BadShippedName(index) => {
                write!(f, "shipped name {index} out of range")
            }
            DecodeError::ShippedNameNotAllowed { name, position } => write!(
                f,
                "the image names {name} as {position}, where no program may name it"
            ),
            DecodeError::ShippedNameInStartedImage(name) => write!(
                f,
                "a started image names {name}, but before main:args: a program reaches \
                 no module's instance and nothing of the shipped world but the \
                 kernel's classes"
            ),
            DecodeError::ShippedInstanceFieldCount { expected, found } => write!(
                f,
                "an instance of a shipped class carries {found} field(s), but the loading \
                 binary lays that class out with {expected}"
            ),
            DecodeError::ShippedInstanceBody { body } => write!(
                f,
                "a record of a shipped class has object body {body}, but an instance of that \
                 class is an instance record"
            ),
            DecodeError::ProgramInstanceFieldCount { expected, found } => write!(
                f,
                "an instance of a program class carries {found} field(s), but its class lays \
                 its instances out with {expected}"
            ),
            DecodeError::ProgramInstanceBody => write!(
                f,
                "an object of a program class is not an instance record, but every instance \
                 of that class is one"
            ),
            DecodeError::NonCanonicalInteger => {
                write!(f, "an integer is not in its one canonical encoding")
            }
            DecodeError::NonNormalFraction => write!(
                f,
                "a fraction is not in normal form: its denominator is not above 1, or its \
                 parts share a factor"
            ),
            DecodeError::FractionRecord => write!(
                f,
                "an object in the image is a Fraction with fields, which only a fraction \
                 value may be"
            ),
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

    /// A String record's text, validated as UTF-8 where it lies and lent
    /// out of the buffer: nothing is allocated for it, so the decoder packs
    /// it straight into the placeholder the first pass sized
    /// (`Heap::fill_placeholder_string`).
    pub(crate) fn read_text(&mut self) -> Result<&'a str, DecodeError> {
        let length = self.read_u32()? as usize;
        std::str::from_utf8(self.read_raw(length)?).map_err(|_| DecodeError::InvalidUtf8)
    }

    /// A `ByteArray` record's bytes, lent out of the buffer: nothing is
    /// allocated for them, and a length past the bytes left is refused
    /// before anything is, so the decoder copies them straight into the
    /// placeholder the first pass sized (`Heap::fill_placeholder_byte_array`).
    pub(crate) fn read_byte_slice(&mut self) -> Result<&'a [u8], DecodeError> {
        let length = self.read_u32()? as usize;
        self.read_raw(length)
    }
}

/// What the header says about the rest of the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Whether the image carries its program's source: the text table, and
    /// per mixin row the index of its text (`FLAG_HAS_SOURCE`). The source
    /// is never executed, and parsed only by an edit after the load.
    pub has_source: bool,
    /// The graph's one root is an application to start (`FLAG_START`).
    pub start: bool,
    /// The shipped world's fingerprint the image was written under; always
    /// this binary's, since a header with another is refused.
    pub fingerprint: u64,
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
    let fingerprint = reader.read_u64()?;
    let expected = super::fingerprint::shipped_world_fingerprint();
    if fingerprint != expected {
        return Err(DecodeError::ShippedWorldMismatch {
            found: fingerprint,
            expected,
        });
    }
    Ok(Header {
        fingerprint,
        has_source: flags & super::encode::FLAG_HAS_SOURCE != 0,
        start: flags & super::encode::FLAG_START != 0,
    })
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
///   `Missing*` errors), because the run-time paths that reach for those
///   bodies panic on the invariant rather than failing, and the absence of
///   a standalone initializer where no source compiles one.
///
/// Methods are checked in selector order, the order the encoder writes a row
/// in, so for any artifact the encoder produced the reported error is the
/// row's first bad entry. `read_row` enforces no order, so for a crafted row
/// that lists its selectors otherwise it is merely one of them.
///
/// `mixins::build` attaches the bodies but checks none of this, so `inspect`
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
        // The same reconciliation for the three initializer bodies, each of
        // whose `argument_count` is the mixin's factory parameter count and
        // whose activation's `arguments` are the factory arguments
        // `instantiate` checked against that same pattern.
        let factory_arity = mixin.factory.map_or(0, |factory| factory.argument_count);
        for (code, what) in [
            (&mixin.initializer_code, "instance initializer"),
            (
                &mixin.own_initializer_code,
                "own part of the instance initializer",
            ),
            (
                &mixin.standalone_initializer_code,
                "standalone instance initializer",
            ),
        ] {
            if let Some(code) = code {
                check_code_arity(code, CodeKind::Initializer, what.to_string(), factory_arity)?;
            }
        }
        // Every declaration compiles a full body, which its class's factory
        // runs on every instantiation.
        if mixin.initializer_code.is_none() {
            return Err(DecodeError::MissingFullInitializer);
        }
        // A level a `<:` clause applies runs the own part of a mixin with
        // slots or `initExpr`s on every instantiation. The row's declaration half says the mixin needs one
        // and its code half says whether the image carries it -- two fields
        // only this function compares.
        if mixin.own_initializer_code.is_none() && instantiate::mixin_needs_initializer_code(mixin)
        {
            return Err(DecodeError::MissingInitializer);
        }
        // A body-less declaration is the mixin application its clauses
        // write (`object-model.md`, "Mixin application"): it has clauses and
        // nothing a body declares.
        if !mixin.has_body {
            if mixin.mixin_applications.is_empty() {
                return Err(DecodeError::BodyLessWithoutClauses);
            }
            if !mixin.slots.is_empty()
                || mixin.has_initializer_expressions
                || !mixin.methods.is_empty()
                || mixin.class_side.is_some()
                || !mixin.declared_nested().is_empty()
            {
                return Err(DecodeError::BodyLessWithMembers);
            }
        }
        // A level of a mixin with `<:` clauses and a body reached from
        // outside runs the standalone body, which only such a mixin has: a
        // body-less declaration's class object is never reached outside it.
        match (
            &mixin.standalone_initializer_code,
            !mixin.mixin_applications.is_empty() && mixin.has_body,
        ) {
            (None, true) => return Err(DecodeError::MissingStandaloneInitializer),
            (Some(_), false) => return Err(DecodeError::UnexpectedStandaloneInitializer),
            _ => {}
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
        if !mixin.mixin_applications.is_empty() && mixin.superclass.is_none() {
            return Err(DecodeError::MixinApplicationWithoutSuperclass);
        }
    }
    check_declaration_code_unshared(mixins)
}

/// No body-less declaration's full initializer is named as any other body
/// (`DecodeError::SharedDeclarationCode`). `mixins::build` marks it, and
/// every block it creates, as the declaration's own code, which starts its
/// lexical sends and `outer` from the declaration's scope; a crafted row
/// that names it again would run it there from another body. A block it
/// creates needs no check: a block has one creator (`SharedBlockBody`), and
/// no row names a block as a body (`check_code_arity`).
fn check_declaration_code_unshared(mixins: &mixins::Decoded) -> Result<(), DecodeError> {
    let mut named: HashMap<usize, usize> = HashMap::new();
    for mixin in &mixins.mixins {
        let bodies = mixin
            .compiled_methods()
            .map(|method| &method.code)
            .chain(mixin.initializer_code.iter())
            .chain(mixin.own_initializer_code.iter())
            .chain(mixin.standalone_initializer_code.iter())
            .chain(mixin.superclass_clause_code.iter());
        for code in bodies {
            if code.is_declaration_code() {
                *named.entry(Rc::as_ptr(code) as usize).or_default() += 1;
            }
        }
    }
    if named.values().any(|count| *count > 1) {
        return Err(DecodeError::SharedDeclarationCode);
    }
    Ok(())
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
// `locals::read_local`/`write_local` index the activation's register window
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
/// and nothing has cloned anything out of the table yet -- `mixins::build`,
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
/// been handed to anything that clones a code out of it (`mixins::build`,
/// today's only such caller) before this runs. Both production callers
/// (`image::load`, `image::inspect::inspect`) call this immediately after
/// `decode_code_table`, before `mixins::build`, which is what makes the
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
/// register operand is inside its own register file and off its header's
/// numbers, no instruction writes an argument register, every send's slots
/// start at or above its body's `local_count` and none of its sources is
/// another of its slots, no instruction reads a register the body may not
/// have written -- what a callee left above a send's slots, or the push in
/// the scratch -- and every outer local is inside the register file of the
/// body it addresses, off that body's header's numbers and below its
/// `local_count` -- off the scratch a live callee of that frame may lie on.
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
    if code.local_count > code.register_count {
        return Err(DecodeError::LocalCountOutOfRange {
            local_count: code.local_count,
            register_count: code.register_count,
        });
    }
    // The VM fetches the instruction at `program_counter` at the top of the
    // loop and again after every fall-through, so an empty body -- or one
    // whose last instruction falls through -- indexes past its end. Not
    // `bytecode::falls_through`'s list, on purpose: a load that always
    // raises ends a path there, but no body may end with it.
    match instructions.last() {
        Some(
            Instruction::Return { .. }
            | Instruction::ImplicitReturn { .. }
            | Instruction::NonLocalReturn { .. }
            | Instruction::SignalNonBoolean { .. }
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
            if names_the_header(code.argument_count, register) {
                return Err(DecodeError::HeaderRegister {
                    depth: 0,
                    register,
                    argument_count: code.argument_count,
                });
            }
        }
        if let Some(register) = register_written(instruction)
            && register < code.argument_count
        {
            return Err(DecodeError::ArgumentRegisterWritten {
                depth: 0,
                register,
                argument_count: code.argument_count,
            });
        }
        // The base and every source are among `own_registers`; the slots
        // past the base are not, and the send writes each one it copies a
        // source into.
        if let Some((argument_base, argument_sources)) = instruction.argument_slots() {
            let end = usize::from(argument_base) + argument_sources.len();
            if end > usize::from(code.register_count) {
                return Err(DecodeError::ArgumentSlotsOutOfRange {
                    argument_base,
                    argument_count: argument_sources.len(),
                    register_count: code.register_count,
                });
            }
            if u32::from(argument_base)
                < u32::from(code.argument_count) + u32::from(HEADER_REGISTERS)
            {
                return Err(DecodeError::ArgumentSlotsBelowTheHeader {
                    argument_base,
                    argument_count: code.argument_count,
                });
            }
            // A block of this body reaches its registers below
            // `local_count` by reference, so a callee's header may lie only
            // at or above it.
            if argument_base < code.local_count {
                return Err(DecodeError::ArgumentSlotsBelowTheScratch {
                    argument_base,
                    local_count: code.local_count,
                });
            }
            for (position, source) in argument_sources.iter().enumerate() {
                let slot = usize::from(*source).checked_sub(usize::from(argument_base));
                if slot.is_some_and(|slot| slot < argument_sources.len() && slot != position) {
                    return Err(DecodeError::SourceIsAnotherSlot {
                        argument_base,
                        position,
                        source: *source,
                    });
                }
            }
        }
        if let Some(target) = jump_target(instruction)
            && target as usize >= instructions.len()
        {
            return Err(DecodeError::JumpTargetOutOfRange {
                target,
                instructions: instructions.len(),
            });
        }
    }
    // Last, on a body every register and target of which is in range: a
    // read of what a callee may have left -- the VM mirror among it -- or
    // of what the push left in the scratch is refused, as no compiled body
    // makes one. The analysis holds one body's bitsets at a time, bounded
    // before it allocates them (`docs/design/memory.md`, "The
    // amplification bound of a rejected load").
    leftovers::check(instructions, code.local_count, code.register_count).map_err(|refusal| {
        match refusal {
            LeftoverRefusal::Read {
                instruction,
                register,
            } => DecodeError::LeftoverRead {
                instruction,
                register,
            },
            LeftoverRefusal::PastTheBound { instruction } => {
                DecodeError::VerificationPastTheBound { instruction }
            }
        }
    })
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
            argument_base,
            argument_sources,
            destination,
            ..
        }
        | Instruction::SendSuper {
            argument_base,
            argument_sources,
            destination,
            ..
        } => {
            let mut registers = argument_sources.clone();
            registers.push(*argument_base);
            registers.push(*destination);
            registers
        }
        Instruction::SendPublic {
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            ..
        }
        | Instruction::SendInlinable {
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            ..
        } => {
            let mut registers = argument_sources.clone();
            registers.push(*argument_base);
            registers.push(*receiver_register);
            registers.push(*destination);
            registers
        }
        // The decoder never produces one -- it has no wire tag -- but the
        // match is total, and answering its real registers keeps it so.
        Instruction::SendMirror {
            mirror_register,
            subject_register,
            argument_base,
            argument_sources,
            destination,
            ..
        } => {
            let mut registers = argument_sources.clone();
            registers.push(*argument_base);
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
        | Instruction::SignalNonBoolean { register }
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
        Instruction::AllocateInstance { destination } => vec![*destination],
        // Its arguments are the body's own, which `argument_count` names
        // and the body's register count was checked against.
        Instruction::InitializeInstance {
            instance,
            destination,
        } => vec![*instance, *destination],
        Instruction::CheckAppliedClause { .. } => vec![],
        Instruction::InitializeSuperclass {
            argument_base,
            argument_sources,
            destination,
            ..
        }
        | Instruction::InitializeAppliedLevel {
            argument_base,
            argument_sources,
            destination,
            ..
        } => {
            let mut registers = argument_sources.clone();
            registers.push(*argument_base);
            registers.push(*destination);
            registers
        }
    }
}

fn verify_outer_locals(
    table: &DecodedCodeTable,
    parents: &[Option<usize>],
    index: usize,
    instructions: &[Instruction],
) -> Result<(), DecodeError> {
    for instruction in instructions {
        let (depth, local, writes) = match instruction {
            Instruction::LoadOuterLocal { depth, index, .. } => (*depth, *index, false),
            Instruction::StoreOuterLocal { depth, index, .. } => (*depth, *index, true),
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
        if names_the_header(enclosing.argument_count, local) {
            return Err(DecodeError::HeaderRegister {
                depth,
                register: local,
                argument_count: enclosing.argument_count,
            });
        }
        if writes && local < enclosing.argument_count {
            return Err(DecodeError::ArgumentRegisterWritten {
                depth,
                register: local,
                argument_count: enclosing.argument_count,
            });
        }
        // The enclosing frame's scratch is where a callee it sends to lies,
        // header and all, while this body runs: a register there is none of
        // its locals.
        if local >= enclosing.local_count {
            return Err(DecodeError::OuterLocalInScratch {
                depth,
                index: local,
                local_count: enclosing.local_count,
            });
        }
    }
    Ok(())
}

/// Whether `register` is one of the numbers the frame header of a body
/// taking `argument_count` arguments takes, which no instruction names
/// (`bytecode::HEADER_REGISTERS`).
fn names_the_header(argument_count: u16, register: u16) -> bool {
    register >= argument_count
        && u32::from(register) < u32::from(argument_count) + u32::from(HEADER_REGISTERS)
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
    // `encode::code_kind_tag`'s numbers; a retired one
    // (`opcodes::RETIRED_CODE_KIND_TAGS`) is refused as never assigned.
    let kind = match reader.read_u8()? {
        5 => CodeKind::Method,
        6 => CodeKind::Block,
        7 => CodeKind::Initializer,
        8 => CodeKind::SuperclassClause,
        tag => {
            return Err(DecodeError::InvalidTag {
                what: "code kind",
                tag,
            });
        }
    };
    let argument_count = reader.read_u16()?;
    check_argument_count("body", usize::from(argument_count))?;
    let register_count = reader.read_u16()?;
    let copied_count = reader.read_u16()?;
    let local_count = reader.read_u16()?;
    // The copied values' own half of the register layout. `argument_count`
    // alone exceeding the file is a different fault, reported as
    // `ArgumentsExceedRegisters` by `verify_body`, so this is asked only
    // once that holds -- and in `u32` arithmetic, so the sum cannot wrap
    // back into range.
    if argument_count <= register_count
        && copied_count > 0
        && u32::from(argument_count) + u32::from(HEADER_REGISTERS) + u32::from(copied_count)
            > u32::from(register_count)
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
    let instruction_count = reader.read_u32()?;
    if instruction_count > INSTRUCTION_LIMIT {
        return Err(DecodeError::InstructionCountTooLarge {
            count: instruction_count,
        });
    }
    let mut builder = CodeBuilder::new();
    for _ in 0..instruction_count {
        builder.emit(decode_instruction(reader, codes, symbols, uncached)?);
    }
    // `from_builder` derives `family_size`, `outer_reach` and
    // `addresses_own_slots` as the compiler's bodies derive them, off the
    // encoding. `family_size` is a placeholder, not
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
        local_count,
        statement_program_counter,
        source,
    ))
}

/// Refuses an arity or argument count past `ARGUMENT_LIMIT`, `what` naming
/// the record it was read from (`DecodeError::ArgumentCountTooLarge`).
pub(crate) fn check_argument_count(what: &'static str, count: usize) -> Result<(), DecodeError> {
    if count > usize::from(ARGUMENT_LIMIT) {
        return Err(DecodeError::ArgumentCountTooLarge { what, count });
    }
    Ok(())
}

fn decode_registers(reader: &mut Reader) -> Result<Vec<u16>, DecodeError> {
    let count = reader.read_u16()? as usize;
    read_registers(reader, count)
}

/// A send's or a clause message's argument registers, `what` naming it:
/// a register list whose count is refused past `ARGUMENT_LIMIT` before
/// anything is reserved for it.
fn decode_arguments(reader: &mut Reader, what: &'static str) -> Result<Vec<u16>, DecodeError> {
    let count = reader.read_u16()? as usize;
    check_argument_count(what, count)?;
    read_registers(reader, count)
}

/// `count` registers, reserved for as the bytes left allow.
fn read_registers(reader: &mut Reader, count: usize) -> Result<Vec<u16>, DecodeError> {
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
            let argument_base = reader.read_u16()?;
            let argument_sources = decode_arguments(reader, "send")?;
            let destination = reader.read_u16()?;
            Instruction::SendLexical {
                depth,
                selector,
                argument_base,
                argument_sources,
                destination,
                call_site,
            }
        }
        OPCODE_SEND_PUBLIC => {
            let selector = decode_symbol(reader, symbols)?;
            let receiver_register = reader.read_u16()?;
            let argument_base = reader.read_u16()?;
            let argument_sources = decode_arguments(reader, "send")?;
            let destination = reader.read_u16()?;
            Instruction::SendPublic {
                selector,
                receiver_register,
                argument_base,
                argument_sources,
                destination,
                call_site,
            }
        }
        OPCODE_SEND_INLINABLE => {
            let selector = decode_symbol(reader, symbols)?;
            let receiver_register = reader.read_u16()?;
            let argument_base = reader.read_u16()?;
            let argument_sources = decode_arguments(reader, "send")?;
            let destination = reader.read_u16()?;
            Instruction::SendInlinable {
                selector,
                receiver_register,
                argument_base,
                argument_sources,
                destination,
                call_site,
            }
        }
        OPCODE_SEND_SUPER => {
            let selector = decode_symbol(reader, symbols)?;
            let argument_base = reader.read_u16()?;
            let argument_sources = decode_arguments(reader, "send")?;
            let destination = reader.read_u16()?;
            Instruction::SendSuper {
                selector,
                argument_base,
                argument_sources,
                destination,
                call_site,
            }
        }
        OPCODE_MAKE_CLOSURE => {
            let destination = reader.read_u16()?;
            let count = reader.read_u16()? as usize;
            check_argument_count("closure", count)?;
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
        OPCODE_SIGNAL_NON_BOOLEAN => Instruction::SignalNonBoolean {
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
        OPCODE_ALLOCATE_INSTANCE => Instruction::AllocateInstance {
            destination: reader.read_u16()?,
        },
        OPCODE_INITIALIZE_INSTANCE => {
            let instance = reader.read_u16()?;
            Instruction::InitializeInstance {
                instance,
                destination: reader.read_u16()?,
            }
        }
        OPCODE_CHECK_APPLIED_CLAUSE => {
            let steps = reader.read_u16()?;
            Instruction::CheckAppliedClause {
                steps,
                selector: decode_symbol(reader, symbols)?,
            }
        }
        OPCODE_INITIALIZE_SUPERCLASS => {
            let steps = reader.read_u16()?;
            let selector = decode_symbol(reader, symbols)?;
            let argument_base = reader.read_u16()?;
            let argument_sources = decode_arguments(reader, "clause message")?;
            Instruction::InitializeSuperclass {
                steps,
                selector,
                argument_base,
                argument_sources,
                destination: reader.read_u16()?,
            }
        }
        OPCODE_INITIALIZE_APPLIED_LEVEL => {
            let steps = reader.read_u16()?;
            let argument_base = reader.read_u16()?;
            let argument_sources = decode_arguments(reader, "clause message")?;
            Instruction::InitializeAppliedLevel {
                steps,
                argument_base,
                argument_sources,
                destination: reader.read_u16()?,
            }
        }
        tag => {
            return Err(DecodeError::InvalidTag {
                what: "instruction",
                tag,
            });
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

/// One part of a `VALUE_FRACTION`: a `VALUE_SMALL_INTEGER` or a
/// `VALUE_LARGE_INTEGER`, nothing else -- a fraction never nests.
fn read_integer_part(reader: &mut Reader) -> Result<ArbitraryInteger, DecodeError> {
    match reader.read_u8()? {
        VALUE_SMALL_INTEGER => Ok(ArbitraryInteger::from_i64(reader.read_i64()?)),
        VALUE_LARGE_INTEGER => read_large_integer(reader),
        tag => Err(DecodeError::InvalidTag {
            what: "fraction part",
            tag,
        }),
    }
}

/// A `VALUE_FRACTION`'s two parts, its tag read: refused unless they are a
/// fraction in normal form (`DecodeError::NonNormalFraction`), which both
/// passes check before anything is allocated for it.
fn read_fraction(reader: &mut Reader) -> Result<Rational, DecodeError> {
    let numerator = read_integer_part(reader)?;
    let denominator = read_integer_part(reader)?;
    if denominator.is_one() || !Rational::is_normal(&numerator, &denominator) {
        return Err(DecodeError::NonNormalFraction);
    }
    Ok(Rational::from_normal_parts(numerator, denominator))
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
            });
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
        LITERAL_BODY_TOO_LARGE => Literal::BodyTooLarge(BodyLimit::Registers),
        LITERAL_BODY_PAST_THE_ARGUMENT_LIMIT => Literal::BodyTooLarge(BodyLimit::Arguments),
        LITERAL_BODY_PAST_THE_INSTRUCTION_LIMIT => Literal::BodyTooLarge(BodyLimit::Instructions),
        tag => {
            return Err(DecodeError::InvalidTag {
                what: "literal",
                tag,
            });
        }
    })
}

// ---------------------------------------------------------------------------
// The object table, frames included.
// ---------------------------------------------------------------------------

/// Everything the graph decoder resolves tags against: the live bootstrap
/// (`mutator`), the object ids of the module roots' records, the decoded
/// code table, the decoded declaration trees a class object's mixin id
/// names, and the image's shipped names resolved against the loading world.
///
/// `mutator` is `&mut` because decoding a class object *allocates into* the
/// interpreter: rebuilding its `MixinApplication` goes through
/// `Mutator::apply_mixin`, and that shared cache is the whole mechanism
/// by which two enclosing instances get back the one application they had
/// before the snapshot.
pub struct DecodeContext<'a, 'gc> {
    pub mutator: &'a mut Mutator<'gc>,
    /// The object id of each module's root class record, in module order,
    /// as the module section names it: module 0's is the class a frame with
    /// no current class resolves its method against (`method_declaration`).
    pub module_roots: &'a [u32],
    pub codes: &'a [Rc<CompiledCode>],
    /// The decoded mixin table's rows, indexed by row id -- the same
    /// `Rc<Mixin>`s that carry the image's compiled bodies and that
    /// `verify_mixins` reconciled, so a class object built around one of
    /// them runs the code the image carries. Using the row's own `Rc` also
    /// matters to the application cache, which keys by `mixin_key`: any
    /// other copy of the same declaration would miss the entry every other
    /// class object of that row shares.
    pub mixins: &'a [Rc<crate::mixin::Mixin>],
    /// The image's shipped-name section, resolved against the loading
    /// world (`shipped_names::resolve`), by index: what a `CLASS_SHIPPED`,
    /// a `MIXIN_SHIPPED`, a `VALUE_SHIPPED_MODULE` and a `HASHED_SHIPPED`
    /// name. Resolved before the graph is decoded, by table lookup, so the
    /// decoder only indexes it and runs no code.
    pub shipped: &'a [ResolvedName<'gc>],
}

pub struct DecodedGraph<'gc> {
    pub roots: Vec<Value<'gc>>,
    /// The largest identity hash of any decoded object -- `load` advances
    /// the interpreter's identity-hash counter past it.
    pub max_identity_hash: u32,
    /// The decoded table itself, in image order, and its frame records
    /// among it, in the same order. The roots reach most of them, but
    /// `verify_graph` must see *every* record -- a frame no root reaches
    /// can still be some closure's captured frame.
    pub objects: Vec<ObjectReference<'gc>>,
    pub frames: Vec<FrameReference<'gc>>,
}

/// The placeholder-allocated table, indexed by the ids in the stream.
pub struct Tables<'a, 'gc> {
    pub objects: &'a [ObjectReference<'gc>],
}

/// The inverse of `encode::encode_class_tag`, at `position`. Tag 2's object
/// may still be a placeholder when this runs -- a class object can be named
/// before its own record is read -- so this resolves identity only; that the
/// record really was a class record is `check_class_references`'s job, once
/// every record has been read. A shipped name exists by then: the loading
/// boot built the shipped world.
pub fn decode_class_tag<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    position: NamePosition,
) -> Result<ClassReference<'gc>, DecodeError> {
    let tag = reader.read_u8()?;
    decode_class_tag_after(tag, reader, context, tables, position)
}

/// An object record's header, as `decode_object_header` read it.
struct ObjectHeader<'gc> {
    class: ClassReference<'gc>,
    /// The header named the shipped `Kernel Activation`.
    activation: bool,
    /// The header named the shipped `Kernel FutureImplementation`.
    future: bool,
    /// The header named a shipped class that is no builtin, whose layout
    /// comes from the loading binary.
    shipped_non_builtin: bool,
    /// The shipped name the header named, if it named one.
    name: Option<ShippedName>,
}

/// An object record's header: its class tag at `NamePosition::Header`.
/// Answers whether it named `Kernel Activation` -- the header of a frame
/// record, the one record that class may have -- or `Kernel
/// FutureImplementation`, the header of an unresolved future's record, so
/// the caller can hold the header and the body to each other
/// (`DecodeError::FrameRecordUnderAnotherClass`, `DecodeError::FutureRecord`)
/// once it has read the body, and whether it named a shipped class that is
/// no builtin, so the caller can hold an instance's fields to that class's
/// layout.
fn decode_object_header<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
) -> Result<ObjectHeader<'gc>, DecodeError> {
    let tag = reader.read_u8()?;
    if tag != CLASS_SHIPPED {
        return Ok(ObjectHeader {
            class: decode_class_tag_after(tag, reader, context, tables, NamePosition::Header)?,
            activation: false,
            future: false,
            shipped_non_builtin: false,
            name: None,
        });
    }
    let entry = read_shipped_class(reader, context.shipped, NamePosition::Header)?;
    let builtin = context.mutator.heap.class_builtin(entry.object);
    Ok(ObjectHeader {
        class: entry.object,
        activation: builtin == Some(BuiltinClass::Activation),
        future: builtin == Some(BuiltinClass::FutureImplementation),
        shipped_non_builtin: builtin.is_none(),
        name: Some(entry.name),
    })
}

/// `decode_class_tag` once its tag byte has been read.
fn decode_class_tag_after<'gc>(
    tag: u8,
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    position: NamePosition,
) -> Result<ClassReference<'gc>, DecodeError> {
    match tag {
        CLASS_RECORD => decode_object_id(reader, tables),
        tag => read_class_tag_outside_the_graph(tag, reader, context, position),
    }
}

/// A class tag after its tag byte, for every tag but `CLASS_RECORD`: the
/// classes the loading world has before the graph, which the first pass
/// resolves as the second does, since none of them is a placeholder.
fn read_class_tag_outside_the_graph<'gc>(
    tag: u8,
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    position: NamePosition,
) -> Result<ClassReference<'gc>, DecodeError> {
    match tag {
        CLASS_SHIPPED => Ok(read_shipped_class(reader, context.shipped, position)?.object),
        tag => Err(DecodeError::InvalidTag { what: "class", tag }),
    }
}

/// A shipped name's index, after its tag, resolved against the image's
/// section: in range, or `BadShippedName` before anything indexes by it.
fn read_shipped_entry<'a, 'gc>(
    reader: &mut Reader,
    shipped: &'a [ResolvedName<'gc>],
) -> Result<&'a ResolvedName<'gc>, DecodeError> {
    let index = reader.read_u32()?;
    shipped
        .get(index as usize)
        .ok_or(DecodeError::BadShippedName(index))
}

/// `read_shipped_entry` where the format types the reference as a class at
/// `position`: the entry must be a class, a metaclass or a level -- a module
/// instance is `NotAClassReference` -- and allowed there
/// (`ShippedNameNotAllowed`).
fn read_shipped_class<'a, 'gc>(
    reader: &mut Reader,
    shipped: &'a [ResolvedName<'gc>],
    position: NamePosition,
) -> Result<&'a ResolvedName<'gc>, DecodeError> {
    let entry = read_shipped_entry(reader, shipped)?;
    if entry.name.kind == ShippedKind::ModuleInstance {
        return Err(DecodeError::NotAClassReference);
    }
    allowed(entry, position)?;
    Ok(entry)
}

/// Refuses `entry` where `position` does not allow it.
fn allowed(entry: &ResolvedName<'_>, position: NamePosition) -> Result<(), DecodeError> {
    if position.allows(entry.positions) {
        Ok(())
    } else {
        Err(DecodeError::ShippedNameNotAllowed {
            name: entry.name.describe(),
            position: position.describe(),
        })
    }
}

fn decode_optional_class<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    position: NamePosition,
) -> Result<Option<ClassReference<'gc>>, DecodeError> {
    if reader.read_boolean()? {
        Ok(Some(decode_class_tag(reader, context, tables, position)?))
    } else {
        Ok(None)
    }
}

/// A `VALUE_SHIPPED_MODULE`'s index, after its tag: an entry a frame's
/// register may hold -- a module instance, the only kind that may stand
/// there -- standing as one (`position` is `NamePosition::FrameRegister`);
/// anywhere else it is refused, since a module instance holds the core
/// library's state.
fn read_shipped_module<'gc>(
    reader: &mut Reader,
    shipped: &[ResolvedName<'gc>],
    position: NamePosition,
) -> Result<ObjectReference<'gc>, DecodeError> {
    let entry = read_shipped_entry(reader, shipped)?;
    allowed(entry, NamePosition::FrameRegister)?;
    allowed(entry, position)?;
    Ok(entry.object)
}

/// One value off the wire. Four kinds allocate objects of their own -- an
/// `i64` beyond 61 bits, a float outside the immediate range, a
/// `LargeInteger` and a `Fraction` -- and that is the one thing the second
/// pass allocates beyond the placeholders the first pass sized: every
/// record is filled into its placeholder in place, each value stored as it
/// is read, a string packed and a byte array's bytes copied straight from
/// the buffer (`Heap::shape_placeholder`, `fill_placeholder_string`,
/// `fill_placeholder_byte_array`, `fill_placeholder_closure`,
/// `fill_frame_fields`), so no record is ever built beside its placeholder.
///
/// The transient peak of a rejected load, against the bytes of the graph
/// section and, for symbols' text, the symbol section
/// (`decode_graph_section` refuses before allocating when the first pass
/// would allocate more than eight words a byte), is the largest of the
/// ratios below; a byte belongs to one record and one value, so they do not
/// add. A placeholder is its allocation in the object space and nothing
/// else: its header word, its class word and its fields, 8 bytes a word in
/// both builds, and an overflow word before the header past 254 words
/// after it, which no record this small reaches. A handle is 8 bytes in a
/// release build and 16 in a debug one, a `Value` 8 and 16.
///
/// - Per record, its object against its smallest record. A metaclass
///   record is the smallest record there is, 9 bytes (a two-byte builtin
///   header, the hash's 4, the body tag, a two-byte builtin class tag),
///   and becomes a class object of 10 words, 80 bytes, 8.9x. A class
///   record is at least 19 bytes for the same 80, 4.2x, and one of a level
///   a `<:` clause added at least 23 (a `MIXIN_SHIPPED` one: the two-byte
///   header, the hash, the body tag, the mixin reference's 5, the
///   superclass's flag, the clause's 8 and the named class's two-byte tag)
///   for the 11 words, 88 bytes, of a level, 3.8x; a body-less
///   declaration's class object (`OBJECT_BODY_LESS_CLASS`) at least 25, a
///   level's 23 and the presence flags of its declaration's enclosing
///   object and declaring class, for 13 words, 104 bytes, 4.2x; a frame record at
///   least 21 (the two-byte header, the hash, the body tag, a `TopLevel`
///   kind, the program counter's 4, four one-byte flags and fields and the
///   register count's 4) for 12 words, 96 bytes, 4.6x; an instance, array,
///   string or symbol record at least 11 for 2 words, 16 bytes, 1.5x; a
///   closure record at least 17 for 5 words, 40 bytes, 2.4x; a byte-array
///   record of `n` bytes is `14 + n` (a five-byte header, the hash, the
///   body tag, the length's 4) for `5 + ⌈n/8⌉` words -- its alignment
///   reserve is not on the wire -- at most 40 bytes for 14, 2.9x; a mixin
///   record at least 13 (the two-byte header, the hash, the body tag, a
///   mixin reference's 5 and the side) for 3 words, 24 bytes, 1.8x.
/// - The decoder's own tables, per record: the placeholder table (a handle,
///   8 or 16 bytes), the first pass's sizes (8 bytes, freed once the
///   placeholders exist) and, in the second pass, a metaclass record's
///   pending index (8), a class record's pending entry (16), a frame
///   record's handle and body offset (8 or 16, and 8). A graph of metaclass
///   records then holds 80 + 8 + 8 = 96 bytes for 9, **10.7x** in a
///   release build, and 80 + 16 + 8 = 104, **11.6x** in a debug one; every
///   other record stays below (a class record 104 or 112 for 19, 5.5x or
///   5.9x; a level's 112 or 120 for 23, 4.9x or 5.2x; a body-less
///   declaration's class object's 128 or 136 for 25, 5.1x or 5.4x; a frame record 120 or
///   136 for 21, 5.7x or 6.5x).
/// - Per value, the word it occupies against the bytes that encode it: a
///   one-byte `VALUE_NIL` field, element, copied value or register becomes
///   an 8-byte word, 8x, which a record of many values approaches from
///   below. A string's UTF-8 byte packed in the 32-bit form costs at most 4
///   bytes, 4x. A symbol record names a symbol-table entry instead, whose
///   text the first pass lets one record allocate: at most 4 bytes per
///   UTF-8 byte of the symbol section.
/// - Per allocating value, its word and its object together: an `i64`
///   beyond 61 bits is 9 bytes (tag, `i64`) for a word and a `LargeInteger`
///   of one digit, 8 + 24 = 32 bytes, 3.6x; a float outside the immediate
///   range the same 32 for its 9 bytes, 3.6x; a `VALUE_LARGE_INTEGER` at
///   least 14 bytes (tag, sign, count, one digit) for the same 32, 2.3x,
///   each further digit 8 bytes for 8. Its digits are held in a `Vec` or a
///   boxed slice first, which lives until its object is made -- the
///   `i64`'s one digit too: one value at a time, 8 bytes a digit more, 4.4x
///   for the `i64` and 2.9x for a one-digit `VALUE_LARGE_INTEGER`. A
///   `VALUE_FRACTION` of two `i64` parts beyond 61 bits is 19 bytes (its
///   tag and two `VALUE_SMALL_INTEGER`s) for the word that holds it, a
///   fraction of 4 words and two one-digit `LargeInteger`s, 8 + 32 + 48 =
///   88 bytes, 4.6x; counting its two transient digits too, each held
///   until its part's object is made, at most 104 bytes, 5.5x. The
///   normal-form check runs before anything is allocated, so its own copy
///   of both parts' digits (`ArbitraryInteger::gcd`) never meets the
///   objects: 16 bytes held and 16 copied, below either figure. Parts
///   written as `VALUE_LARGE_INTEGER`s cost more bytes for the same
///   objects, each further digit 8 bytes for at most 16.
/// - The root list, read after every record into `Value`s (`reserve_for`
///   caps its reservation at the bytes left): 8 bytes a one-byte root in a
///   release build, 8x, and 16 in a debug one, **16x**.
///
/// So the peak is about **10.7x** in a release build (a graph of
/// metaclass records) and **16x** in a debug one (a graph that is all root
/// list), plus at most 4x of the symbol section and, in an image with
/// source, 6x of the text table (`texts::decode`). Not counted: each Rust
/// allocation's own overhead in the allocator; a decoder table's `Vec`
/// doubling as it grows, which holds both buffers for a moment; and the
/// object space's commit growth, which commits at least double what it had
/// committed, so the space may have up to about twice what the
/// placeholders occupy committed, though not touched.
fn decode_value<'gc>(
    reader: &mut Reader,
    context: &mut DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    position: NamePosition,
) -> Result<Value<'gc>, DecodeError> {
    match reader.read_u8()? {
        VALUE_NIL => Ok(Value::nil()),
        VALUE_FALSE => Ok(Value::boolean(false)),
        VALUE_TRUE => Ok(Value::boolean(true)),
        // An `i64` beyond 61 bits allocates its `LargeInteger`: 4.4x of its
        // 9 bytes with the word that holds it and its transient digit
        // (above).
        VALUE_SMALL_INTEGER => Ok(integer_value(context.mutator, reader.read_i64()?)),
        // A float outside the immediate range allocates its boxed `Float`:
        // 3.6x of its 9 bytes with the word that holds it (above).
        VALUE_FLOAT => Ok(context.mutator.new_float(reader.read_f64()?)),
        VALUE_OBJECT_ID => Ok(Value::object(decode_object_id(reader, tables)?)),
        VALUE_CLASS_TAG => Ok(Value::object(decode_class_tag(
            reader,
            context,
            tables,
            NamePosition::Value,
        )?)),
        VALUE_SHIPPED_MODULE => Ok(Value::object(read_shipped_module(
            reader,
            context.shipped,
            position,
        )?)),
        // Allocating here is sound: the decoder never polls a safepoint, so
        // nothing moves the placeholders `tables` holds. It stays
        // proportional to the input because the digit count is bounded
        // against the bytes left before anything is allocated for it
        // (`read_large_integer_parts`), and each further digit costs its
        // own 8 wire bytes.
        VALUE_LARGE_INTEGER => {
            let integer = read_large_integer(reader)?;
            Ok(context.mutator.new_integer(integer))
        }
        // Its two parts allocate as `VALUE_SMALL_INTEGER` and
        // `VALUE_LARGE_INTEGER` do, and the fraction four words more, once
        // the normal form is checked; its ratio to its bytes stays below
        // the bound above.
        VALUE_FRACTION => {
            let fraction = read_fraction(reader)?;
            Ok(context.mutator.new_fraction(fraction))
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
        values.push(decode_value(reader, context, tables, NamePosition::Value)?);
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

/// A frame reference off the wire: an object id, resolved like any other,
/// whose record must be a frame record. Every frame record's placeholder is
/// a frame object of its size from the start (`decode_graph_section`), so
/// whether the id names one is known at once, forward references included.
fn decode_frame_id<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
) -> Result<FrameReference<'gc>, DecodeError> {
    let id = reader.read_u32()?;
    let object = tables
        .objects
        .get(id as usize)
        .copied()
        .ok_or(DecodeError::BadObjectId(id))?;
    context
        .mutator
        .heap
        .object_frame(object)
        .ok_or(DecodeError::NotAFrameReference(id))
}

fn decode_frame_reference<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
) -> Result<Option<FrameReference<'gc>>, DecodeError> {
    if reader.read_boolean()? {
        Ok(Some(decode_frame_id(reader, context, tables)?))
    } else {
        Ok(None)
    }
}

/// A closure record, filled into `object`, its placeholder, in place: its
/// header and fixed fields, then each copied value as it is read.
fn decode_closure_record<'gc>(
    reader: &mut Reader,
    context: &mut DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    object: ObjectReference<'gc>,
    class: ClassReference<'gc>,
    identity_hash: u32,
) -> Result<(), DecodeError> {
    let code_id = reader.read_u32()?;
    let code = context
        .codes
        .get(code_id as usize)
        .cloned()
        .ok_or(DecodeError::BadCodeId(code_id))?;
    let captured_frame = decode_frame_id(reader, context, tables)?;
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
    // Its feedback base is never on the wire: a decoded closure is given a
    // fresh range of its family's size when it is first activated
    // (`Mutator::push_step`).
    context.mutator.heap.fill_placeholder_closure(
        object,
        class,
        identity_hash,
        &code,
        captured_frame,
    );
    decode_fields_into(
        reader,
        context,
        tables,
        object,
        record_size::CLOSURE_FIXED_FIELDS,
        count,
    )
}

/// `count` values off the wire, each stored into `object`'s field
/// `first + index` as it is read: a placeholder the first pass sized for
/// exactly these values, so nothing is collected beside it. A field past
/// the placeholder -- which the first pass, reading the same bytes, rules
/// out -- is refused rather than indexed.
fn decode_fields_into<'gc>(
    reader: &mut Reader,
    context: &mut DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    object: ObjectReference<'gc>,
    first: usize,
    count: usize,
) -> Result<(), DecodeError> {
    for index in 0..count {
        let value = decode_value(reader, context, tables, NamePosition::Value)?;
        let field = first.saturating_add(index);
        if !context
            .mutator
            .heap
            .fill_placeholder_field(object, field, value)
        {
            return Err(DecodeError::UnexpectedEndOfFile);
        }
    }
    Ok(())
}

/// Re-resolves a `FrameKind::Method` against the decoded class's method
/// map: the image names the method by selector and body, never by
/// identity, and the method found is the frame's when its body is the one
/// the frame names (`frame_method`).
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
/// application's module, module 0, stands in: the record the module section
/// names as its root class, as its root class did for every frame before
/// nesting. The frames are decoded after every class record has its
/// application, so that record answers its own; whether it really is the
/// module's root is `modules::module_root_classes`'s to refuse, and a record
/// that is no class here answers no method.
fn method_declaration<'gc>(
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    current_class: Option<&ClassReference<'gc>>,
    selector: Symbol,
) -> Result<Rc<crate::mixin::MixinMethod>, DecodeError> {
    let class = match current_class {
        Some(class) => *class,
        None => {
            let id = *context.module_roots.first().ok_or(DecodeError::NotAClass)?;
            tables
                .objects
                .get(id as usize)
                .copied()
                .ok_or(DecodeError::BadObjectId(id))?
        }
    };
    let found = context
        .mutator
        .heap
        .class_application(class)
        .and_then(|application| application.mixin.compiled_method(selector).cloned());
    found.ok_or(DecodeError::UnknownSelector(selector.as_str().to_string()))
}

/// How a frame record named its body (`opcodes::FRAME_BODY_*`), as the
/// second pass read it.
enum FrameBody {
    None,
    Code(Rc<CompiledCode>),
    Declared,
}

/// A frame kind whose body a shipped declaration may carry, with the class
/// it resolves through: what `FRAME_BODY_DECLARED` names, read the same way
/// by the encoder, which decides whether to write it, and the decoder,
/// which finds the body by it.
pub(crate) enum DeclaredKind<'gc> {
    /// A method of the current class's mixin, by selector.
    Method(ClassReference<'gc>, Symbol),
    Initializer(ClassReference<'gc>, instantiate::InitializerBody),
    /// A factory's body, derived from the class it runs on, whatever
    /// declared it.
    Factory(ClassReference<'gc>),
    SuperclassClause(ClassReference<'gc>),
}

impl<'gc> DeclaredKind<'gc> {
    /// `kind`'s, with `current_class` for a method; `None` for a closure or
    /// a top-level frame, and a method frame with no current class, none of
    /// which a declaration names.
    pub(crate) fn of_kind(
        kind: &FrameKind<'gc>,
        current_class: Option<ClassReference<'gc>>,
    ) -> Option<DeclaredKind<'gc>> {
        Some(match kind {
            FrameKind::Method(method) => DeclaredKind::Method(current_class?, method.selector),
            FrameKind::Initializer(class, body) => DeclaredKind::Initializer(*class, *body),
            FrameKind::Factory(class) => DeclaredKind::Factory(*class),
            FrameKind::SuperclassClause(class) => DeclaredKind::SuperclassClause(*class),
            FrameKind::Closure(_) | FrameKind::TopLevel => return None,
        })
    }

    /// The declaration that carries the body: the mixin of the class it
    /// resolves through -- for an initializer, the mixin whose body a frame
    /// of it runs (`Heap::initializer_mixin`), which for a body-less
    /// declaration's full body is the declaration's, not the applied
    /// mixin's.
    fn declaration(&self, heap: &Heap<'gc>) -> Option<Rc<crate::mixin::Mixin>> {
        match self {
            DeclaredKind::Initializer(class, body) => heap.initializer_mixin(*class, *body),
            DeclaredKind::Method(class, _)
            | DeclaredKind::Factory(class)
            | DeclaredKind::SuperclassClause(class) => heap
                .class_application(*class)
                .map(|application| application.mixin.clone()),
        }
    }

    /// Whether the body is a shipped declaration's: exactly when a frame
    /// names it by declaration.
    pub(crate) fn is_shipped(&self, heap: &Heap<'gc>) -> bool {
        self.declaration(heap)
            .is_some_and(|declaration| declaration.shipped)
    }
}

/// The method `selector` names in the shipped mixin `class` applies: what
/// a method frame written `FRAME_BODY_DECLARED` runs. Refused when the
/// mixin is no shipped one, since every other body is named by code id.
fn declared_method<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    selector: Symbol,
) -> Result<Rc<crate::mixin::MixinMethod>, DecodeError> {
    let declared = DeclaredKind::Method(class, selector);
    if !declared.is_shipped(heap) {
        return Err(DecodeError::DeclaredBodyNotShipped);
    }
    declared
        .declaration(heap)
        .and_then(|mixin| mixin.compiled_method(selector).cloned())
        .ok_or(DecodeError::UnknownSelector(selector.as_str().to_string()))
}

/// The body a frame written `FRAME_BODY_DECLARED` runs, found through its
/// kind in the shipped declaration it resolves through
/// (`DeclaredKind::declaration`). Refused when that declaration is no
/// shipped one, or carries no body of the kind. A factory's body is
/// derived from the class it runs on, whichever declaration that is: its
/// metaclass's factory entry, which no image carries either.
pub(crate) fn declared_body<'gc>(
    heap: &Heap<'gc>,
    declared: &DeclaredKind<'gc>,
) -> Result<Rc<CompiledCode>, DecodeError> {
    let found = match declared {
        DeclaredKind::Factory(class) => instantiate::factory_code(heap, *class),
        _ if !declared.is_shipped(heap) => return Err(DecodeError::DeclaredBodyNotShipped),
        DeclaredKind::Method(class, selector) => {
            return declared_method(heap, *class, *selector).map(|method| method.code.clone());
        }
        DeclaredKind::Initializer(_, body) => declared
            .declaration(heap)
            .and_then(|mixin| body.code_of(&mixin)),
        DeclaredKind::SuperclassClause(_) => declared
            .declaration(heap)
            .and_then(|mixin| mixin.superclass_clause_code.clone()),
    };
    found.ok_or(DecodeError::FrameCodeMismatch)
}

/// A frame's body off the wire: a `u32` code id, checked against the code
/// table before it indexes it.
fn frame_code(
    reader: &mut Reader,
    context: &DecodeContext<'_, '_>,
) -> Result<Rc<CompiledCode>, DecodeError> {
    let code_id = reader.read_u32()?;
    context
        .codes
        .get(code_id as usize)
        .cloned()
        .ok_or(DecodeError::BadCodeId(code_id))
}

/// The method a decoded method frame runs: the current declaration's method
/// of `selector` when its body is `code` (`method_declaration`), so that
/// the frame names the very method dispatch finds, and otherwise one built
/// over `code` -- the method of a version an install superseded, which the
/// frame ran before the edit. `verify_graph` holds `code` to being a method
/// body of the method's argument count.
fn frame_method<'gc>(
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    current_class: Option<&ClassReference<'gc>>,
    selector: Symbol,
    access: ns_syntax::ast::AccessModifier,
    code: &Rc<CompiledCode>,
) -> Rc<crate::mixin::MixinMethod> {
    match method_declaration(context, tables, current_class, selector) {
        Ok(method) if Rc::ptr_eq(&method.code, code) => method,
        _ => Rc::new(crate::mixin::MixinMethod {
            selector,
            access,
            argument_count: usize::from(code.argument_count),
            // A version an install superseded: no declaration, and no dispatch
            // finds it, since a decoded frame is never entered again.
            declaration: None,
            code: Rc::clone(code),
            fast_path: std::cell::Cell::new(None),
            recognized_fast_path: None,
            class_accessor: None,
        }),
    }
}

/// An `OBJECT_FRAME` body (`opcodes::OBJECT_FRAME` gives the layout), filled
/// into `frame`, its placeholder, in place: the fixed fields, then each
/// register as it is read. Its registers arrive as one list: a frame's
/// arguments are its code's, so the record carries no split, and
/// `verify_graph` holds the count to the body.
fn decode_frame<'gc>(
    reader: &mut Reader,
    context: &mut DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    symbols: &symbols::Table,
    frame: FrameReference<'gc>,
) -> Result<(), DecodeError> {
    // A method is named by selector and access, resolved once its body and
    // the current class that declares it have been read, below: until then
    // the kind is the selector and access (`Err`), every other kind already
    // itself (`Ok`).
    let kind = match reader.read_u8()? {
        FRAME_METHOD => Err((
            decode_symbol(reader, symbols)?,
            mixins::access_from_tag(reader.read_u8()?)?,
        )),
        FRAME_TOP_LEVEL => Ok(FrameKind::TopLevel),
        FRAME_FACTORY => Ok(FrameKind::Factory(decode_class_tag(
            reader,
            context,
            tables,
            NamePosition::FrameClass,
        )?)),
        FRAME_SUPERCLASS_CLAUSE => Ok(FrameKind::SuperclassClause(decode_class_tag(
            reader,
            context,
            tables,
            NamePosition::FrameClass,
        )?)),
        FRAME_CLOSURE => Ok(FrameKind::Closure(decode_object_id(reader, tables)?)),
        tag => match instantiate::InitializerBody::of_frame_tag(tag) {
            Some(body) => Ok(FrameKind::Initializer(
                decode_class_tag(reader, context, tables, NamePosition::FrameClass)?,
                body,
            )),
            None => {
                return Err(DecodeError::InvalidTag {
                    what: "frame kind",
                    tag,
                });
            }
        },
    };
    let top_level = matches!(kind, Ok(FrameKind::TopLevel));
    // `None` for a body named by declaration, found below once the current
    // class is read.
    let body = match reader.read_u8()? {
        // A top-level frame runs no body and every other kind runs one: a
        // frame without one would be reconciled against nothing.
        FRAME_BODY_NONE if top_level => FrameBody::None,
        FRAME_BODY_NONE => return Err(DecodeError::FrameCodeMismatch),
        FRAME_BODY_CODE if top_level || matches!(kind, Ok(FrameKind::Factory(_))) => {
            return Err(DecodeError::FrameCodeMismatch);
        }
        FRAME_BODY_CODE => FrameBody::Code(frame_code(reader, context)?),
        FRAME_BODY_DECLARED if top_level || matches!(kind, Ok(FrameKind::Closure(_))) => {
            return Err(DecodeError::DeclaredBodyNotShipped);
        }
        FRAME_BODY_DECLARED => FrameBody::Declared,
        tag => {
            return Err(DecodeError::InvalidTag {
                what: "frame body",
                tag,
            });
        }
    };
    let program_counter = reader.read_u32()? as usize;
    let uncontinuable = reader.read_boolean()?;
    // An image is only written while no activation runs, so every frame it
    // carries has returned. A frame not marked so could be resumed from its
    // crafted registers and program counter, which nothing here has run.
    if !uncontinuable {
        return Err(DecodeError::ContinuableFrame);
    }
    let receiver = decode_value(reader, context, tables, NamePosition::FrameRegister)?;
    let sender = decode_frame_reference(reader, context, tables)?;
    let home = decode_frame_reference(reader, context, tables)?;
    let current_class = decode_optional_class(reader, context, tables, NamePosition::FrameClass)?;
    let (kind, code) = match (kind, body) {
        (Ok(kind), FrameBody::None) => (kind, None),
        (Ok(kind), FrameBody::Code(code)) => (kind, Some(code)),
        (Ok(kind), FrameBody::Declared) => {
            let declared = DeclaredKind::of_kind(&kind, current_class)
                .ok_or(DecodeError::DeclaredBodyNotShipped)?;
            let code = declared_body(&context.mutator.heap, &declared)?;
            (kind, Some(code))
        }
        (Err((selector, access)), FrameBody::Code(code)) => (
            FrameKind::Method(frame_method(
                context,
                tables,
                current_class.as_ref(),
                selector,
                access,
                &code,
            )),
            Some(code),
        ),
        (Err((selector, _)), FrameBody::Declared) => {
            let class = current_class.ok_or(DecodeError::DeclaredBodyNotShipped)?;
            let method = declared_method(&context.mutator.heap, class, selector)?;
            let code = Rc::clone(&method.code);
            (FrameKind::Method(method), Some(code))
        }
        (Err(_), FrameBody::None) => return Err(DecodeError::FrameCodeMismatch),
    };
    context.mutator.heap.fill_frame_fields(
        frame,
        FrameRecord {
            receiver,
            arguments: Vec::new(),
            temporaries: Vec::new(),
            program_counter,
            sender,
            home,
            current_class,
            kind,
            uncontinuable,
        },
        code,
    );
    let count = reader.read_u32()? as usize;
    for index in 0..count {
        let value = decode_value(reader, context, tables, NamePosition::FrameRegister)?;
        if !context
            .mutator
            .heap
            .fill_frame_register(frame, index, value)
        {
            return Err(DecodeError::UnexpectedEndOfFile);
        }
    }
    Ok(())
}

/// A floor under the smallest object record the wire admits: a class tag of
/// at least one byte (every live one is followed by at least one more), an
/// identity hash (4), and a body tag (1). Every body adds at least two more
/// bytes, so this is a floor, not a typical size: what `bounded_count`
/// holds the object count to before the first pass reserves its table of
/// sizes. A lower bound stays a sound one.
pub(crate) const MIN_OBJECT_RECORD_BYTES: usize = 6;

/// A floor under the smallest entry of the closing hash table: a tag (1) and
/// a hash (4). A shipped entry, the one kind there is, carries its `u32`
/// index besides; a lower bound stays a sound one.
pub(crate) const MIN_TAG_NAMED_HASH_BYTES: usize = 5;

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
/// chain to its already-applied end (a shipped class, or a class
/// resolved by an earlier walk) over an explicit stack, then applies back
/// down it. A chain that closes on itself is `SuperclassCycle` rather than
/// a loop that never ends.
fn rebuild_applications<'gc>(
    context: &mut DecodeContext<'_, 'gc>,
    objects: &[ObjectReference<'gc>],
    pending: Vec<PendingClass>,
    pending_metaclasses: &[usize],
) -> Result<(), DecodeError> {
    // Class-object identity key -> position in `pending`, for the "is my
    // superclass still waiting for its own application?" question each step
    // asks. Keyed by identity because a superclass arrives as a `ClassReference`
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
    // wire -- a decoded record starts with no verdict
    // (`Heap::fill_placeholder_class` gives it none). A legitimate snapshot can hold a class that was
    // created carrying one (or one built of decoded pieces would carry,
    // `Counter`-over-`Holder`-style), so it is recomputed here, exactly as
    // `after_clause` decides it for a class created from source -- never for
    // a metaclass or an applied level other than a body-less declaration's
    // class object, which `after_clause` does not check either, and only when the application inherits `Value`,
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
        let heap = &context.mutator.heap;
        let own_class = !heap.class_is_applied_level(object)
            || heap.class_body_less_declaration(object).is_some();
        if !application.inherits_value || !own_class {
            continue;
        }
        // A body-less declaration's class object is decided as
        // `nested::body_less_class` decides it, on its declaration's
        // enclosing object and its level's both; a root's, whose scope is
        // empty, is never decided, as no root is.
        if let Some(scope) = heap.class_declaration_scope(object) {
            let Some(declaration_enclosing) = scope.enclosing else {
                continue;
            };
            if let Some(refusal) = crate::interpreter::value_class::check_body_less_value_class(
                context.mutator,
                declaration_enclosing,
                object,
            ) && !context.mutator.record_value_class_refusal(object, refusal)
            {
                unreachable!("internal: a pending class record fills a class object");
            }
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
        ) && !context.mutator.record_value_class_refusal(object, refusal)
        {
            unreachable!("internal: a pending class record fills a class object");
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
/// exactly those. A shipped metaclass is named by its shipped name and
/// already exists; one of a metaclass or of an ordinary object does not
/// exist at all.
fn rebuild_metaclasses<'gc>(
    context: &mut DecodeContext<'_, 'gc>,
    objects: &[ObjectReference<'gc>],
    pending: &[PendingClass],
    position: &HashMap<usize, usize>,
    pending_metaclasses: &[usize],
) -> Result<(), DecodeError> {
    let class_sides = class_sides(context, objects, pending, position)?;
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
        let class_side_mixin = class_sides[this_position].clone();
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

/// The class side each pending class record's metaclass applies, chosen as
/// a live class's is (`nested::build_applied_levels`): a body-less
/// declaration's own for its class object, the class side of the class its
/// clause named for any other level a `<:` clause added -- read off that
/// class's metaclass when the loading world has it, and chosen the same way
/// when it is another record -- and `Mutator::metaclass_mixin`'s of the
/// record's own row otherwise.
///
/// Linear in the records: each one's answer is computed once and kept for
/// every level whose chain of named classes reaches it. A chain that closes
/// on itself is `NamedClassCycle`, never a loop that does not end.
fn class_sides<'gc>(
    context: &mut DecodeContext<'_, 'gc>,
    objects: &[ObjectReference<'gc>],
    pending: &[PendingClass],
    position: &HashMap<usize, usize>,
) -> Result<Vec<Rc<crate::mixin::Mixin>>, DecodeError> {
    let mut resolved: Vec<Option<Rc<crate::mixin::Mixin>>> = vec![None; pending.len()];
    let mut on_chain = vec![false; pending.len()];
    let mut chain: Vec<usize> = Vec::new();
    for start in 0..pending.len() {
        let mut current = start;
        let answer = loop {
            if let Some(found) = &resolved[current] {
                break found.clone();
            }
            if on_chain[current] {
                return Err(DecodeError::NamedClassCycle);
            }
            on_chain[current] = true;
            chain.push(current);
            let object = objects[pending[current].object];
            let heap = &context.mutator.heap;
            if let Some(declaration) = heap.class_body_less_declaration(object).cloned() {
                break context.mutator.metaclass_mixin(&declaration);
            }
            let named = heap
                .class_named(object)
                .filter(|_| heap.class_is_applied_level(object));
            let Some(named) = named else {
                break context.mutator.metaclass_mixin(&pending[current].mixin);
            };
            if let Some(&next) = position.get(&heap.object_identity_key(named)) {
                current = next;
                continue;
            }
            // A class the loading world has before the graph: its
            // metaclass exists already.
            let built = heap
                .class_metaclass(named)
                .and_then(|metaclass| heap.class_application(metaclass))
                .map(|class_side| class_side.mixin.clone());
            break match built {
                Some(class_side) => class_side,
                None => context.mutator.metaclass_mixin(&pending[current].mixin),
            };
        };
        for index in chain.drain(..) {
            on_chain[index] = false;
            resolved[index] = Some(answer.clone());
        }
    }
    Ok(resolved
        .into_iter()
        .map(|class_side| class_side.expect("every pending class was resolved above"))
        .collect())
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

/// Every object of a class that is no builtin and whose chain does not
/// reach `Behavior` -- whose instances are class objects, which
/// `check_class_references` holds -- is an instance record of exactly its
/// class's application's slot count, as every such instance is allocated
/// (`instantiate::allocate_for_factory`). The shipped classes' records
/// were held to it as they were read; a program class's application
/// exists only once `rebuild_applications` has run, so its instances are
/// checked here, after it. What the check guards: a customized copy reads
/// its receiver's own slots at fixed fields with no check
/// (`jit::optimizing::graph::Node::OwnSlotLoad`).
fn check_program_instances<'gc>(
    heap: &Heap<'gc>,
    objects: &[ObjectReference<'gc>],
) -> Result<(), DecodeError> {
    for object in objects {
        let class = heap.object_class(*object);
        if heap.class_builtin(class).is_some() {
            continue;
        }
        let application = heap.class_application(class);
        let class_object_class = application.as_ref().is_some_and(|application| {
            matches!(
                application.instantiation_refusal,
                Some(crate::mixin::InstantiationRefusal::ClassObject)
            )
        });
        if class_object_class {
            continue;
        }
        let found = match heap.object_shape(*object) {
            crate::heap::ObjectShape::Instance => heap
                .instance_field_count(*object)
                .ok_or(DecodeError::ProgramInstanceBody)?,
            _ => return Err(DecodeError::ProgramInstanceBody),
        };
        let expected = application.map_or(0, |application| application.slot_count);
        if found != expected {
            return Err(DecodeError::ProgramInstanceFieldCount { expected, found });
        }
    }
    Ok(())
}

/// Every field the format types as a class reference really names a class
/// object, and a class object's own `class` is its metaclass or, for a
/// metaclass, `Metaclass`.
///
/// A tag-2 class tag names an object-table id, and nothing in the byte
/// stream forces that record to have been a class record -- which is only
/// knowable once every record has been read, so it is checked here rather
/// than where each tag is decoded. These positions, a level's named class
/// (`Heap::class_named`) and a body-less declaration's declaring class
/// (`Heap::class_declaration_scope`) among them, are the complete set: a class tag appears nowhere else in the graph section, and a plain
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
            let named = heap.class_named(*object);
            let declaration_declaring = heap
                .class_declaration_scope(*object)
                .and_then(|scope| scope.declaring_class);
            for class in superclass
                .iter()
                .chain(declaring_class.iter())
                .chain(named.iter())
                .chain(declaration_declaring.iter())
            {
                if !heap.is_class(*class) {
                    return Err(DecodeError::NotAClassReference);
                }
            }
        }
    }
    for frame in frames {
        let current_class = heap.frame_current_class(*frame);
        let kind = heap.frame_kind(*frame);
        if let FrameKind::Closure(closure) = &kind
            && !heap.is_closure(*closure)
        {
            return Err(DecodeError::NotAClosureReference);
        }
        let kind_class = match &kind {
            FrameKind::Initializer(class, _)
            | FrameKind::Factory(class)
            | FrameKind::SuperclassClause(class) => Some(class),
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

/// A record as the first pass sized it: the fields of the object it becomes
/// -- a frame's fixed fields and registers, a class object's fields, a
/// string's field words -- and whether it is a frame record, whose
/// placeholder is a frame object from the start (`decode_frame_id`). One
/// word apiece, the least the table between the two passes can cost.
#[derive(Clone, Copy)]
struct PlannedRecord(u64);

impl PlannedRecord {
    fn new(fields: usize, frame: bool) -> PlannedRecord {
        PlannedRecord(((fields as u64) << 1) | u64::from(frame))
    }

    fn fields(self) -> usize {
        usize::try_from(self.0 >> 1).unwrap_or(usize::MAX)
    }

    fn frame(self) -> bool {
        self.0 & 1 == 1
    }
}

/// The first pass over the graph section: reads every record's header and
/// body as the second pass will, storing nothing -- values are parsed and
/// discarded, object ids are range-checked against the object count, and
/// the tags that name a class the loading world already has are resolved
/// as the second pass resolves them -- so that every record's size is known
/// before anything is allocated. It allocates nothing in the heap.
struct Sizing<'pass, 'a, 'gc> {
    context: &'pass DecodeContext<'a, 'gc>,
    symbols: &'pass symbols::Table,
    object_count: usize,
}

impl Sizing<'_, '_, '_> {
    fn object_id(&self, reader: &mut Reader) -> Result<(), DecodeError> {
        let id = reader.read_u32()?;
        if id as usize >= self.object_count {
            return Err(DecodeError::BadObjectId(id));
        }
        Ok(())
    }

    fn class_tag(&self, reader: &mut Reader, position: NamePosition) -> Result<(), DecodeError> {
        let tag = reader.read_u8()?;
        self.class_tag_after(tag, reader, position)
    }

    fn class_tag_after(
        &self,
        tag: u8,
        reader: &mut Reader,
        position: NamePosition,
    ) -> Result<(), DecodeError> {
        match tag {
            CLASS_RECORD => self.object_id(reader),
            tag => read_class_tag_outside_the_graph(tag, reader, self.context, position).map(drop),
        }
    }

    fn optional_class(
        &self,
        reader: &mut Reader,
        position: NamePosition,
    ) -> Result<(), DecodeError> {
        if reader.read_boolean()? {
            self.class_tag(reader, position)?;
        }
        Ok(())
    }

    /// `decode_object_header`'s reading; answers the builtin the header
    /// named, if it named a shipped builtin, and whether it named a shipped
    /// class that is no builtin.
    fn header(&self, reader: &mut Reader) -> Result<(Option<BuiltinClass>, bool), DecodeError> {
        let tag = reader.read_u8()?;
        if tag != CLASS_SHIPPED {
            return self
                .class_tag_after(tag, reader, NamePosition::Header)
                .map(|()| (None, false));
        }
        let entry = read_shipped_class(reader, self.context.shipped, NamePosition::Header)?;
        let builtin = self.context.mutator.heap.class_builtin(entry.object);
        Ok((builtin, builtin.is_none()))
    }

    /// `decode_value`'s reading. A `LargeInteger`'s digits, and a
    /// fraction's parts, are read into a transient `Vec`, bounded by the
    /// bytes they occupy, and dropped.
    fn value(&self, reader: &mut Reader, position: NamePosition) -> Result<(), DecodeError> {
        match reader.read_u8()? {
            VALUE_NIL | VALUE_FALSE | VALUE_TRUE => Ok(()),
            VALUE_SMALL_INTEGER => reader.read_i64().map(drop),
            VALUE_FLOAT => reader.read_f64().map(drop),
            VALUE_OBJECT_ID => self.object_id(reader),
            VALUE_CLASS_TAG => self.class_tag(reader, NamePosition::Value),
            VALUE_SHIPPED_MODULE => {
                read_shipped_module(reader, self.context.shipped, position).map(drop)
            }
            VALUE_LARGE_INTEGER => read_large_integer(reader).map(drop),
            // Its normal form checked here too, so an image that carries
            // one outside it is refused before anything is allocated.
            VALUE_FRACTION => read_fraction(reader).map(drop),
            tag => Err(DecodeError::InvalidTag { what: "value", tag }),
        }
    }

    /// A counted list of values; answers the count, which every value read
    /// has paid for in bytes.
    fn values(&self, reader: &mut Reader, position: NamePosition) -> Result<usize, DecodeError> {
        let count = reader.read_u32()? as usize;
        for _ in 0..count {
            self.value(reader, position)?;
        }
        Ok(count)
    }

    fn frame_reference(&self, reader: &mut Reader) -> Result<(), DecodeError> {
        if reader.read_boolean()? {
            self.object_id(reader)?;
        }
        Ok(())
    }

    /// `decode_class_record`'s reading; answers the record's field count, a
    /// level's one more than a class object's (`Heap::allocate_applied_level`).
    fn class_record(&self, reader: &mut Reader) -> Result<usize, DecodeError> {
        match reader.read_u8()? {
            MIXIN_ROW => {
                let id = reader.read_u32()?;
                if self.context.mixins.get(id as usize).is_none() {
                    return Err(DecodeError::BadMixinId(id));
                }
                self.optional_class(reader, NamePosition::Superclass)?;
                if reader.read_boolean()? {
                    self.value(reader, NamePosition::Value)?;
                }
                self.optional_class(reader, NamePosition::FrameClass)?;
                if reader.read_boolean()? {
                    decode_applied_clause(reader, self.context, None)?;
                    self.class_tag(reader, NamePosition::Superclass)?;
                    return Ok(record_size::APPLIED_LEVEL_FIELDS);
                }
                Ok(record_size::CLASS_FIELDS)
            }
            MIXIN_SHIPPED => {
                let named = read_shipped_mixin(reader, self.context.shipped)?;
                self.optional_class(reader, NamePosition::Superclass)?;
                let builtin = self.context.mutator.heap.class_builtin(named);
                decode_applied_clause(reader, self.context, builtin)?;
                self.class_tag(reader, NamePosition::Superclass)?;
                Ok(record_size::APPLIED_LEVEL_FIELDS)
            }
            tag => Err(DecodeError::InvalidTag {
                what: "mixin reference",
                tag,
            }),
        }
    }

    /// `decode_frame`'s reading; answers the register count.
    fn frame_body(&self, reader: &mut Reader) -> Result<usize, DecodeError> {
        match reader.read_u8()? {
            FRAME_METHOD => {
                decode_symbol(reader, self.symbols)?;
                mixins::access_from_tag(reader.read_u8()?)?;
            }
            FRAME_TOP_LEVEL => {}
            FRAME_INITIALIZER
            | FRAME_OWN_INITIALIZER
            | FRAME_STANDALONE_INITIALIZER
            | FRAME_FACTORY
            | FRAME_SUPERCLASS_CLAUSE => self.class_tag(reader, NamePosition::FrameClass)?,
            FRAME_CLOSURE => self.object_id(reader)?,
            tag => {
                return Err(DecodeError::InvalidTag {
                    what: "frame kind",
                    tag,
                });
            }
        }
        match reader.read_u8()? {
            FRAME_BODY_NONE | FRAME_BODY_DECLARED => {}
            FRAME_BODY_CODE => {
                frame_code(reader, self.context)?;
            }
            tag => {
                return Err(DecodeError::InvalidTag {
                    what: "frame body",
                    tag,
                });
            }
        }
        reader.read_u32()?; // the program counter, checked by `verify_graph`

        // Refused here, before any placeholder exists; the fill pass checks
        // again, so no path skips the refusal.
        if !reader.read_boolean()? {
            return Err(DecodeError::ContinuableFrame);
        }
        self.value(reader, NamePosition::FrameRegister)?; // the receiver
        self.frame_reference(reader)?; // the sender
        self.frame_reference(reader)?; // the home
        self.optional_class(reader, NamePosition::FrameClass)?; // the current class
        self.values(reader, NamePosition::FrameRegister)
    }

    /// One record, sized: what it becomes, and the words of it the size
    /// check counts -- all of them but a symbol's text, which the symbol
    /// section bounds instead. A symbol record names a symbol-table entry
    /// rather than carrying its text, so many short records could name one
    /// long entry; refusing a second record naming the same entry, or one
    /// the heap already holds, before allocating keeps each entry's text
    /// allocated at most once, and bounded by its own encoding there.
    ///
    /// A mixin record's mixin is read and held to what a reified mixin may
    /// name, and pushed onto `reified` for `decode_graph_section` to
    /// resolve and hold to being the only one of its mixin, still before
    /// anything is allocated.
    fn record(
        &self,
        reader: &mut Reader,
        symbols_named: &mut [bool],
        reified: &mut Vec<(Rc<crate::mixin::Mixin>, bool)>,
    ) -> Result<(PlannedRecord, usize), DecodeError> {
        let (header, shipped_non_builtin) = self.header(reader)?;
        let future = header == Some(BuiltinClass::FutureImplementation);
        let fraction = header == Some(BuiltinClass::Fraction);
        reader.read_u32()?; // the identity hash, checked by the second pass
        let body = reader.read_u8()?;
        // A shipped class that is no builtin lays its instances out with
        // slots, as the loading binary declares them: a record of any other
        // shape under its header would make an object its code reads as
        // something it is not. Refused before anything is allocated, as the
        // second pass would refuse it.
        if shipped_non_builtin && body != OBJECT_INSTANCE {
            return Err(DecodeError::ShippedInstanceBody { body });
        }
        // `Fraction new`'s record is the one a `Fraction` header may carry:
        // a fraction is a `VALUE_FRACTION`, its normal form checked, and a
        // record of fields under the header would make one unchecked. The
        // first pass reads every record, so this refuses it before
        // anything is allocated.
        if fraction && body != OBJECT_INSTANCE {
            return Err(DecodeError::FractionRecord);
        }
        // Only `ByteArray`'s header carries a byte-array record, and it
        // carries no other. A header that names a record of the graph is
        // a program's class, never `ByteArray`, so the header is known
        // here whatever it names.
        if (header == Some(BuiltinClass::ByteArray)) != (body == OBJECT_BYTE_ARRAY) {
            return Err(DecodeError::ByteArrayRecordMismatch { body });
        }
        // Likewise `WeakArray`'s and `Ephemeron`'s, except that `WeakArray`'s
        // header carries an instance record too, `withoutSlots`'s, whose
        // count the arm below holds to the class's.
        let weak_array = header == Some(BuiltinClass::WeakArray);
        if weak_array_record_mismatch(weak_array, body) {
            return Err(DecodeError::WeakArrayRecordMismatch { body });
        }
        if (header == Some(BuiltinClass::Ephemeron)) != (body == OBJECT_EPHEMERON) {
            return Err(DecodeError::EphemeronRecordMismatch { body });
        }
        // Likewise `Mixin`'s: an object of it stands for a mixin only as
        // the table's one object of that mixin, which only a mixin record
        // makes.
        if (header == Some(BuiltinClass::Mixin)) != (body == OBJECT_MIXIN) {
            return Err(DecodeError::MixinRecordMismatch { body });
        }
        let (fields, frame) = match body {
            // A future has exactly its two fields; a record under its header
            // claiming any other count is refused here, before it is
            // allocated, as the second pass would refuse it once read.
            OBJECT_INSTANCE if future => {
                let count = self.values(reader, NamePosition::Value)?;
                if count != crate::interpreter::future::FIELD_COUNT {
                    return Err(DecodeError::FutureRecord);
                }
                (count, false)
            }
            OBJECT_INSTANCE if fraction => {
                let count = self.values(reader, NamePosition::Value)?;
                if count != 0 {
                    return Err(DecodeError::FractionRecord);
                }
                (count, false)
            }
            OBJECT_INSTANCE if weak_array => {
                let count = self.values(reader, NamePosition::Value)?;
                weak_array_instance_field_count(self.context.mutator, count)?;
                (count, false)
            }
            OBJECT_INSTANCE | OBJECT_ARRAY | OBJECT_WEAK_ARRAY => {
                (self.values(reader, NamePosition::Value)?, false)
            }
            // Its count checked before its values are read, as the second
            // pass checks it, so a mismatch is reported as itself.
            OBJECT_EPHEMERON => {
                let count = reader.read_u32()? as usize;
                ephemeron_field_count(self.context.mutator, count)?;
                for _ in 0..count {
                    self.value(reader, NamePosition::Value)?;
                }
                (count, false)
            }
            OBJECT_STRING => {
                let text = reader.read_text()?;
                (record_size::string_fields(text), false)
            }
            OBJECT_BYTE_ARRAY => (
                record_size::byte_array_fields(reader.read_byte_slice()?.len()),
                false,
            ),
            OBJECT_SYMBOL => {
                let id = reader.read_u32()?;
                let symbol = self.symbols.get(id)?;
                let named = symbols_named
                    .get_mut(id as usize)
                    .ok_or(DecodeError::BadSymbolId(id))?;
                if *named || self.context.mutator.heap.has_symbol_object(symbol) {
                    return Err(DecodeError::DuplicateSymbol(symbol.as_str().to_string()));
                }
                *named = true;
                let planned =
                    PlannedRecord::new(record_size::string_fields(symbol.as_str()), false);
                return Ok((planned, record_size::block_words(0)));
            }
            OBJECT_CLOSURE => {
                let code_id = reader.read_u32()?;
                let code = self
                    .context
                    .codes
                    .get(code_id as usize)
                    .ok_or(DecodeError::BadCodeId(code_id))?;
                self.object_id(reader)?;
                let count = reader.read_u16()? as usize;
                // Checked before the values are read, as the second pass
                // checks it, so a mismatch is reported as itself.
                if count != code.copied_count as usize {
                    return Err(DecodeError::CopiedCountMismatch {
                        copied: count,
                        copied_count: code.copied_count,
                    });
                }
                for _ in 0..count {
                    self.value(reader, NamePosition::Value)?;
                }
                (record_size::CLOSURE_FIXED_FIELDS + count, false)
            }
            OBJECT_CLASS => (self.class_record(reader)?, false),
            // A level's record and the declaration's scope; whether the
            // clause is a body-less declaration's last is the second pass's
            // to check, which the placeholder's size then agrees with.
            OBJECT_BODY_LESS_CLASS => {
                if self.class_record(reader)? != record_size::APPLIED_LEVEL_FIELDS {
                    return Err(DecodeError::BodyLessClassRecordMismatch);
                }
                if reader.read_boolean()? {
                    self.value(reader, NamePosition::Value)?;
                }
                self.optional_class(reader, NamePosition::FrameClass)?;
                (record_size::BODY_LESS_CLASS_FIELDS, false)
            }
            OBJECT_METACLASS => {
                self.class_tag(reader, NamePosition::Header)?;
                (record_size::CLASS_FIELDS, false)
            }
            OBJECT_MIXIN => {
                reified.push(read_mixin_record(reader, self.context)?);
                (record_size::MIXIN_FIELDS, false)
            }
            OBJECT_FRAME => {
                let registers = self.frame_body(reader)?;
                (
                    record_size::FRAME_FIXED_FIELDS.saturating_add(registers),
                    true,
                )
            }
            tag => {
                return Err(DecodeError::InvalidTag {
                    what: "object body",
                    tag,
                });
            }
        };
        Ok((
            PlannedRecord::new(fields, frame),
            record_size::block_words(fields),
        ))
    }
}

/// Refuses an ephemeron record of `count` fields unless `Ephemeron`
/// declares exactly that many: its methods read its own slots, the key
/// first, and the collectors read the first field as the key.
fn ephemeron_field_count(mutator: &Mutator<'_>, count: usize) -> Result<(), DecodeError> {
    let expected = builtin_slot_count(mutator, BuiltinClass::Ephemeron);
    if count != expected {
        return Err(DecodeError::EphemeronFieldCount {
            expected,
            found: count,
        });
    }
    Ok(())
}

/// Whether a record of `body` is refused for `WeakArray`'s header, which
/// `weak_array` says it is: a weak-array record goes under that header
/// alone, and that header carries nothing else but `withoutSlots`'s
/// instance record.
fn weak_array_record_mismatch(weak_array: bool, body: u8) -> bool {
    if weak_array {
        body != OBJECT_WEAK_ARRAY && body != OBJECT_INSTANCE
    } else {
        body == OBJECT_WEAK_ARRAY
    }
}

/// Refuses an instance record of `count` fields under `WeakArray`'s
/// header unless the class declares exactly that many slots, as the
/// instance `withoutSlots` makes has.
fn weak_array_instance_field_count(mutator: &Mutator<'_>, count: usize) -> Result<(), DecodeError> {
    let expected = builtin_slot_count(mutator, BuiltinClass::WeakArray);
    if count != expected {
        return Err(DecodeError::WeakArrayInstanceFieldCount {
            expected,
            found: count,
        });
    }
    Ok(())
}

/// How many slots the builtin class `kind` declares, as the loading
/// binary lays it out.
fn builtin_slot_count(mutator: &Mutator<'_>, kind: BuiltinClass) -> usize {
    let class = mutator.builtin_class(kind);
    mutator
        .heap
        .class_application(class)
        .map_or(0, |application| application.slot_count)
}

/// An `OBJECT_MIXIN` body, after its tag: the instance side its reference
/// names, as a class record names its mixin, and whether the side byte
/// names that mixin's class side. Refuses a row past the table, a shipped
/// name that is no class's, a class side's own row and a side other than
/// 0 or 1. It reads and looks up only, so the first pass and the second
/// both call it; `resolve_reified_mixin` takes the side.
fn read_mixin_record(
    reader: &mut Reader,
    context: &DecodeContext<'_, '_>,
) -> Result<(Rc<crate::mixin::Mixin>, bool), DecodeError> {
    let mixin = match reader.read_u8()? {
        MIXIN_ROW => {
            let id = reader.read_u32()?;
            context
                .mixins
                .get(id as usize)
                .cloned()
                .ok_or(DecodeError::BadMixinId(id))?
        }
        MIXIN_SHIPPED => {
            let entry = read_shipped_entry(reader, context.shipped)?;
            if entry.name.kind != ShippedKind::Class {
                return Err(DecodeError::MixinRecordNamesNoClass(entry.name.describe()));
            }
            allowed(entry, NamePosition::Mixin)?;
            context
                .mutator
                .heap
                .class_application(entry.object)
                .map(|application| application.mixin.clone())
                .ok_or(DecodeError::NotAClass)?
        }
        tag => {
            return Err(DecodeError::InvalidTag {
                what: "mixin reference",
                tag,
            });
        }
    };
    if mixin.is_class_side() {
        return Err(DecodeError::MixinRecordNamesAClassSide);
    }
    let class_side = match reader.read_u8()? {
        MIXIN_SIDE_INSTANCE => false,
        MIXIN_SIDE_CLASS => true,
        side => return Err(DecodeError::BadMixinSide(side)),
    };
    Ok((mixin, class_side))
}

/// The mixin an `OBJECT_MIXIN` record stands for: `instance_side`, or with
/// `class_side` its class side -- the declared one, or the one
/// `Mutator::metaclass_mixin` keeps for a class that declares none, which
/// it synthesizes on first demand as a Rust value, allocating nothing in
/// the object space. The first pass resolves every record's, so the second
/// only looks it up again.
fn resolve_reified_mixin(
    mutator: &mut Mutator<'_>,
    instance_side: &Rc<crate::mixin::Mixin>,
    class_side: bool,
) -> Rc<crate::mixin::Mixin> {
    if class_side {
        mutator.metaclass_mixin(instance_side)
    } else {
        instance_side.clone()
    }
}

/// The graph section in two passes, so that cycles and shared references
/// round-trip with identity preserved and nothing is allocated before the
/// whole table has been sized against the bytes that carry it.
///
/// The first pass (`Sizing`) reads every record without storing anything
/// and learns the size of the object each becomes. Every record is
/// allocated at that size before the second pass reads the records again
/// and fills them, so a forward reference names an object that already
/// exists, and a frame reference can be held to naming a frame record the
/// moment it is read.
///
/// Class objects need one more step: their records are filled in like any
/// other, but the `MixinApplication` a class object carries is derived
/// rather than stored, and deriving it needs the superclass's -- which may
/// itself be a record that appears later. `rebuild_applications` closes that
/// gap once every record has been read. The frame records are filled after
/// it, from where the second pass found them, since a `FRAME_METHOD` frame
/// looks its selector up in `current_class`'s method dictionary to reuse
/// the method there when it runs the frame's body (`frame_method`).
///
/// Every pass is a flat loop over the table, and every reference between
/// records is an id into it, never an inline nested record -- so no part of
/// this decoder recurses on a depth the image controls.
pub fn decode_graph_section<'gc>(
    reader: &mut Reader,
    context: &mut DecodeContext<'_, 'gc>,
    symbols: &symbols::Table,
) -> Result<DecodedGraph<'gc>, DecodeError> {
    let object_count = bounded_count(reader.read_u32()? as usize, MIN_OBJECT_RECORD_BYTES, reader)?;
    let records_start = reader.position();
    let mut planned: Vec<PlannedRecord> = Vec::with_capacity(object_count);
    let mut counted_words: usize = 0;
    let mut reified = Vec::new();
    {
        let sizing = Sizing {
            context,
            symbols,
            object_count,
        };
        let mut symbols_named = vec![false; symbols.all().len()];
        for _ in 0..object_count {
            let (record, words) = sizing.record(reader, &mut symbols_named, &mut reified)?;
            planned.push(record);
            counted_words = counted_words.saturating_add(words);
        }
    }
    // A heap holds one object per mixin, and so does an image of one: a
    // second record for a mixin, or one for a mixin this heap reified
    // already, is refused before anything is allocated, as a symbol's is.
    // Resolving a class side may synthesize it, a Rust value only. The
    // list and the set are dropped before the placeholders exist.
    {
        let mut reified_ids = HashSet::new();
        for (instance_side, class_side) in reified {
            let mixin = resolve_reified_mixin(context.mutator, &instance_side, class_side);
            let id = context.mutator.heap.mixin_id(&mixin);
            if !reified_ids.insert(id) || context.mutator.heap.mixin_object(id).is_some() {
                return Err(DecodeError::DuplicateMixin(mixin.reflected_name()));
            }
        }
    }
    // The premise of the load's amplification bound (`decode_value` has the
    // figures): what the placeholders occupy is proportional to the bytes
    // that describe them, at most eight words a byte of the section and
    // what follows it. Every value the first pass read cost a byte, so a
    // well-formed record stays well inside it; this refuses, before
    // anything is allocated, whatever a later change to the layout lets
    // slip past that.
    let bytes_described = reader.position() - records_start + reader.remaining();
    if counted_words > bytes_described.saturating_mul(8) {
        return Err(DecodeError::UnexpectedEndOfFile);
    }
    *reader = Reader {
        buffer: reader.buffer,
        position: records_start,
    };

    let placeholder_class = context.mutator.builtin_class(BuiltinClass::Object);
    // A real application, standing in until `rebuild_applications` derives
    // each class object's own -- the same technique as the placeholder
    // objects, and for the same reason: nothing may observe a half-built
    // record, and a `None` here would need an optional application on a
    // class object that no live class object could ever have.
    let placeholder_application = context
        .mutator
        .heap
        .class_application(placeholder_class)
        .ok_or(DecodeError::NotAClass)?;
    let mut objects: Vec<ObjectReference> = Vec::with_capacity(object_count);
    for record in planned {
        let heap = &mut context.mutator.heap;
        objects.push(if record.frame() {
            let registers = record
                .fields()
                .saturating_sub(record_size::FRAME_FIXED_FIELDS);
            let frame = heap.allocate_placeholder_frame(registers);
            heap.frame_object(frame)
        } else {
            heap.allocate_placeholder(placeholder_class, record.fields())
        });
    }
    let tables = Tables { objects: &objects };
    let mut max_identity_hash = 0;
    let mut pending_classes: Vec<PendingClass> = Vec::new();
    let mut pending_metaclasses: Vec<usize> = Vec::new();
    let mut decoded_a_future = false;
    // Each frame record's frame and where its body starts, for the fill
    // after `rebuild_applications`.
    let mut frames: Vec<FrameReference<'gc>> = Vec::new();
    let mut frame_bodies: Vec<usize> = Vec::new();
    let metaclass_rows = metaclass_rows(context.mixins);
    let metaclass_class = context.mutator.builtin_class(BuiltinClass::Metaclass);
    let symbol_class = context.mutator.builtin_class(BuiltinClass::Symbol);
    for (index, object) in objects.iter().enumerate() {
        let ObjectHeader {
            class,
            activation: activation_header,
            future: future_header,
            shipped_non_builtin,
            name: header_name,
        } = decode_object_header(reader, context, &tables)?;
        let identity_hash = reader.read_u32()?;
        // Checked here, the one place an object record's hash is read: the
        // header word holds 22 bits, and `load` hands the maximum straight
        // to `advance_identity_hash_past`.
        if identity_hash >= IDENTITY_HASH_LIMIT {
            return Err(DecodeError::IdentityHashTooWide(identity_hash));
        }
        max_identity_hash = max_identity_hash.max(identity_hash);
        let body = reader.read_u8()?;
        // The first pass refused it already; restated here, beside the
        // layout check below, as what filling the record rests on.
        if shipped_non_builtin && body != OBJECT_INSTANCE {
            return Err(DecodeError::ShippedInstanceBody { body });
        }
        // Refused by the first pass too; restated here as what filling a
        // byte-array record, and every other record under `ByteArray`'s
        // header, rests on.
        if (context.mutator.heap.class_builtin(class) == Some(BuiltinClass::ByteArray))
            != (body == OBJECT_BYTE_ARRAY)
        {
            return Err(DecodeError::ByteArrayRecordMismatch { body });
        }
        // Refused by the first pass too; restated here as what filling a
        // weak-array or an ephemeron record rests on.
        let builtin = context.mutator.heap.class_builtin(class);
        let weak_array = builtin == Some(BuiltinClass::WeakArray);
        if weak_array_record_mismatch(weak_array, body) {
            return Err(DecodeError::WeakArrayRecordMismatch { body });
        }
        if (builtin == Some(BuiltinClass::Ephemeron)) != (body == OBJECT_EPHEMERON) {
            return Err(DecodeError::EphemeronRecordMismatch { body });
        }
        if (builtin == Some(BuiltinClass::Mixin)) != (body == OBJECT_MIXIN) {
            return Err(DecodeError::MixinRecordMismatch { body });
        }
        if body == OBJECT_FRAME {
            if !activation_header {
                return Err(DecodeError::FrameRecordUnderAnotherClass);
            }
            let heap = &mut context.mutator.heap;
            let frame = heap
                .object_frame(*object)
                .ok_or(DecodeError::NotAFrameReference(
                    u32::try_from(index).unwrap_or(u32::MAX),
                ))?;
            heap.object_set_identity_hash(*object, identity_hash);
            frames.push(frame);
            frame_bodies.push(reader.position());
            Sizing {
                context,
                symbols,
                object_count,
            }
            .frame_body(reader)?;
            continue;
        }
        // `Activation` names a frame record's class and nothing else's: on
        // any other record it would make an ordinary object answer
        // `Activation`'s methods, which read other frames and unwind the
        // stack.
        if activation_header {
            return Err(DecodeError::ShippedNameNotAllowed {
                name: header_name.map_or_else(String::new, |name| name.describe()),
                position: NamePosition::Header.describe(),
            });
        }
        if future_header && body != OBJECT_INSTANCE {
            return Err(DecodeError::FutureRecord);
        }
        // Set only for a class record a `<:` clause added and for a
        // metaclass record, each written once the record is filled.
        let mut applied_clause = None;
        let mut this_class = None;
        let mut declaration_scope = None;
        // `class` may name a record later in this same table -- decoding
        // fills records in table order, not dependency order -- so that
        // class may not have an application yet. The record's header id is
        // `NONE`, which matches no cache entry; `stamp_dispatch_ids`, right
        // after `rebuild_applications` gives every class one, replaces it
        // with the real id.
        //
        // Every record but a class object's is filled in place: its values
        // are stored into the placeholder the first pass sized as they are
        // read, so nothing beside it is built (`decode_value` has the
        // figures).
        let mut decoded_symbol = None;
        let record = match body {
            OBJECT_INSTANCE => {
                let count = reader.read_u32()? as usize;
                // The loading binary lays a shipped class out, not the image,
                // so an instance of one must fit that layout exactly.
                // The first pass refused any other count under the
                // `FutureImplementation` header; restated here, beside the
                // layout check, as what reading the two fields below rests on.
                if future_header && count != crate::interpreter::future::FIELD_COUNT {
                    return Err(DecodeError::FutureRecord);
                }
                // Refused by the first pass too; restated here as what
                // `WeakArray`'s leaves declining on the instance rests on.
                if weak_array {
                    weak_array_instance_field_count(context.mutator, count)?;
                }
                if shipped_non_builtin {
                    let expected = context
                        .mutator
                        .heap
                        .class_application(class)
                        .map_or(0, |application| application.slot_count);
                    if count != expected {
                        return Err(DecodeError::ShippedInstanceFieldCount {
                            expected,
                            found: count,
                        });
                    }
                }
                context.mutator.heap.shape_placeholder(
                    *object,
                    class,
                    identity_hash,
                    PlaceholderFormat::Instance,
                );
                decode_fields_into(reader, context, &tables, *object, 0, count)?;
                // Held to what the encoder writes; that its computation is
                // a closure is checked below, once the record it names has
                // been read.
                if future_header {
                    let heap = &context.mutator.heap;
                    let fields: [Value<'gc>; crate::interpreter::future::FIELD_COUNT] =
                        std::array::from_fn(|index| {
                            heap.instance_field(*object, index)
                                .unwrap_or_else(Value::nil)
                        });
                    if !crate::interpreter::future::is_a_decoded_future_record(&fields) {
                        return Err(DecodeError::FutureRecord);
                    }
                    decoded_a_future = true;
                }
                None
            }
            OBJECT_STRING => {
                if class.same(symbol_class) {
                    return Err(DecodeError::StringOfSymbolClass);
                }
                let text = reader.read_text()?;
                if !context.mutator.heap.fill_placeholder_string(
                    *object,
                    class,
                    identity_hash,
                    text,
                ) {
                    return Err(DecodeError::UnexpectedEndOfFile);
                }
                None
            }
            OBJECT_BYTE_ARRAY => {
                let bytes = reader.read_byte_slice()?;
                // The first pass sized the placeholder from these bytes; a
                // placeholder of another size is refused, as a string's is.
                if !context.mutator.heap.fill_placeholder_byte_array(
                    *object,
                    class,
                    identity_hash,
                    bytes,
                ) {
                    return Err(DecodeError::UnexpectedEndOfFile);
                }
                None
            }
            OBJECT_SYMBOL => {
                let symbol = decode_symbol(reader, symbols)?;
                if !context.mutator.heap.fill_placeholder_string(
                    *object,
                    class,
                    identity_hash,
                    symbol.as_str(),
                ) {
                    return Err(DecodeError::UnexpectedEndOfFile);
                }
                decoded_symbol = Some(symbol);
                None
            }
            OBJECT_ARRAY => {
                let count = reader.read_u32()? as usize;
                context.mutator.heap.shape_placeholder(
                    *object,
                    class,
                    identity_hash,
                    PlaceholderFormat::Array,
                );
                decode_fields_into(reader, context, &tables, *object, 0, count)?;
                None
            }
            OBJECT_WEAK_ARRAY => {
                let count = reader.read_u32()? as usize;
                context.mutator.heap.shape_placeholder(
                    *object,
                    class,
                    identity_hash,
                    PlaceholderFormat::WeakArray,
                );
                decode_fields_into(reader, context, &tables, *object, 0, count)?;
                None
            }
            OBJECT_EPHEMERON => {
                let count = reader.read_u32()? as usize;
                ephemeron_field_count(context.mutator, count)?;
                context.mutator.heap.shape_placeholder(
                    *object,
                    class,
                    identity_hash,
                    PlaceholderFormat::Ephemeron,
                );
                decode_fields_into(reader, context, &tables, *object, 0, count)?;
                None
            }
            OBJECT_CLOSURE => {
                decode_closure_record(reader, context, &tables, *object, class, identity_hash)?;
                None
            }
            // Recorded as soon as it is filled, so that the table holds it
            // before anything reads it back (`Heap::mixin_object_id`). The
            // first pass refused a second record for its mixin; restated
            // here as what recording it rests on.
            OBJECT_MIXIN => {
                let (instance_side, class_side) = read_mixin_record(reader, context)?;
                let mixin = resolve_reified_mixin(context.mutator, &instance_side, class_side);
                let heap = &mut context.mutator.heap;
                let id = heap.mixin_id(&mixin);
                if heap.mixin_object(id).is_some() {
                    return Err(DecodeError::DuplicateMixin(mixin.reflected_name()));
                }
                heap.shape_placeholder(*object, class, identity_hash, PlaceholderFormat::Instance);
                let index = i64::try_from(id.index()).expect("a mixin id is 32 bits");
                if !heap.fill_placeholder_field(*object, 0, Value::small_integer(index)) {
                    return Err(DecodeError::UnexpectedEndOfFile);
                }
                heap.record_mixin_object(id, *object);
                None
            }
            body @ (OBJECT_CLASS | OBJECT_BODY_LESS_CLASS) => {
                let (class_record, applied, mixin) =
                    decode_class_record(reader, context, &tables, &placeholder_application)?;
                // The tag says whether the record carries a declaration's
                // scope, the clause whether the class object has one.
                let body_less = applied.as_ref().is_some_and(|(clause, _)| {
                    !clause.declaration.has_body
                        && clause.index + 1 == clause.declaration.mixin_applications.len()
                });
                if body_less != (body == OBJECT_BODY_LESS_CLASS) {
                    return Err(DecodeError::BodyLessClassRecordMismatch);
                }
                if body_less {
                    declaration_scope = Some(decode_declaration_scope(reader, context, &tables)?);
                }
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
                applied_clause = applied;
                Some(class_record)
            }
            OBJECT_METACLASS => {
                // §3.2: every metaclass is an instance of `Metaclass`.
                if !class.same(metaclass_class) {
                    return Err(DecodeError::MetaclassPairing);
                }
                this_class = Some(decode_class_tag(
                    reader,
                    context,
                    &tables,
                    NamePosition::Header,
                )?);
                let class_class = context.mutator.builtin_class(BuiltinClass::Class);
                pending_metaclasses.push(index);
                Some(NewClass {
                    application: placeholder_application.clone(),
                    enclosing: None,
                    declaring_class: None,
                    superclass: Some(class_class),
                    builtin: None,
                })
            }
            tag => {
                return Err(DecodeError::InvalidTag {
                    what: "object body",
                    tag,
                });
            }
        };
        let heap = &mut context.mutator.heap;
        if let Some(record) = record {
            // A placeholder the first pass sized for some other record is
            // refused, as a string's of the wrong size is: the space cannot
            // resize it.
            let named = applied_clause.as_ref().map(|(_, named)| *named);
            if !heap.fill_placeholder_class(
                *object,
                class,
                identity_hash,
                record,
                named,
                declaration_scope,
            ) {
                return Err(DecodeError::UnexpectedEndOfFile);
            }
        }
        if let Some((applied, _)) = applied_clause
            && !heap.class_set_applied_clause(*object, applied)
        {
            unreachable!("internal: a class record fills a class object");
        }
        if let Some(this_class) = this_class
            && !heap.class_set_this_class(*object, this_class)
        {
            unreachable!("internal: a metaclass record fills a class object");
        }
        // A heap holds one object per symbol, and so does an image of one:
        // a second record of the same text is refused, not merged
        // (`docs/design/image-format.md`, "Verification"). The first pass
        // refused it before anything was allocated (`Sizing::record`); it
        // is restated here, beside the class check, as what entering the
        // canonical table rests on. `load` decodes into a fresh
        // interpreter, where nothing made a symbol before the graph
        // section, so an entry already here came from this graph.
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
            if context.mutator.heap.has_symbol_object(symbol) {
                return Err(DecodeError::DuplicateSymbol(symbol.as_str().to_string()));
            }
            context
                .mutator
                .heap
                .record_canonical_symbol(symbol, *object);
        }
    }
    // Every check for a future stays off a program's paths until one exists
    // (`Mutator::futures_made`); a loaded one exists as surely as a made one.
    if decoded_a_future {
        context.mutator.note_futures_made();
    }
    rebuild_applications(context, &objects, pending_classes, &pending_metaclasses)?;
    context.mutator.heap.stamp_dispatch_ids(&objects);
    for (frame, body) in frames.iter().zip(frame_bodies) {
        let mut body_reader = Reader {
            buffer: reader.buffer,
            position: body,
        };
        decode_frame(&mut body_reader, context, &tables, symbols, *frame)?;
    }
    check_class_references(&context.mutator.heap, &objects, &frames)?;
    check_program_instances(&context.mutator.heap, &objects)?;
    let roots = decode_values(reader, context, &tables)?;
    Ok(DecodedGraph {
        roots,
        max_identity_hash,
        objects,
        frames,
    })
}

/// The inverse of `encode::encode_tag_named_hashes`: puts each recorded hash
/// back onto the shipped object the loading boot built,
/// and answers the largest one, which `load` advances the counter past
/// together with the graph's own maximum. A shipped entry may name any kind
/// of shipped object, a module instance included: a hash is no reference,
/// and none of them is a record. A 0, a repeated object and a hash of 2^22 or
/// more (`IDENTITY_HASH_LIMIT`) are refused, each for the reason its
/// `DecodeError` gives.
pub fn decode_tag_named_hashes<'gc>(
    mutator: &mut Mutator<'gc>,
    reader: &mut Reader,
    shipped: &[ResolvedName<'gc>],
) -> Result<u32, DecodeError> {
    let count = bounded_count(
        reader.read_u32()? as usize,
        MIN_TAG_NAMED_HASH_BYTES,
        reader,
    )?;
    // One flag per section entry: `resolve` refuses a name listed twice, and
    // the shipped world names each object once
    // (`an_object_and_a_mixin_lead_back_to_their_entry`), so a section index
    // is an object, and a repeat is found without comparing objects.
    let mut restored = vec![false; shipped.len()];
    let mut max = 0;
    for _ in 0..count {
        let index = match reader.read_u8()? {
            HASHED_SHIPPED => {
                let index = reader.read_u32()?;
                if shipped.get(index as usize).is_none() {
                    return Err(DecodeError::BadShippedName(index));
                }
                index as usize
            }
            tag => {
                return Err(DecodeError::InvalidTag {
                    what: "tag-named hash",
                    tag,
                });
            }
        };
        let hash = reader.read_u32()?;
        if hash == 0 {
            return Err(DecodeError::UnassignedTagNamedHash);
        }
        if hash >= IDENTITY_HASH_LIMIT {
            return Err(DecodeError::IdentityHashTooWide(hash));
        }
        if std::mem::replace(&mut restored[index], true) {
            return Err(DecodeError::DuplicateTagNamedHash);
        }
        mutator
            .heap
            .object_set_identity_hash(shipped[index].object, hash);
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
/// ever run a clause `verify_mixins` reconciled with its row's
/// factory.
///
/// A level whose clause named a shipped class (`MIXIN_SHIPPED`) is its own
/// shape: its mixin, enclosing object and declaring class are the loading
/// world's class's, exactly what `nested::build_applied_levels` gives a
/// level it builds, so the record carries only the name, the superclass and
/// the clause. The three references resolve to identities that already
/// exist (every record was allocated at its size before this pass started),
/// so a forward reference and a cycle are both ordinary; what cannot be
/// settled yet is whether each really is a class record, which
/// `check_class_references` establishes afterwards.
/// A class record as `decode_class_record` reads it: the class object to
/// build, the `<:` clause that added it, if any, set once it is built
/// (`Heap::class_set_applied_clause`), and the mixin-table row it applies.
type DecodedClassRecord<'gc> = (
    NewClass<'gc>,
    Option<(crate::mixin::AppliedClause, ClassReference<'gc>)>,
    Rc<crate::mixin::Mixin>,
);

fn decode_class_record<'gc>(
    reader: &mut Reader,
    context: &mut DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    placeholder_application: &Rc<crate::mixin::MixinApplication>,
) -> Result<DecodedClassRecord<'gc>, DecodeError> {
    let mixin = match reader.read_u8()? {
        MIXIN_ROW => {
            let id = reader.read_u32()?;
            context
                .mixins
                .get(id as usize)
                .cloned()
                .ok_or(DecodeError::BadMixinId(id))?
        }
        MIXIN_SHIPPED => {
            return decode_shipped_level(reader, context, tables, placeholder_application);
        }
        tag => {
            return Err(DecodeError::InvalidTag {
                what: "mixin reference",
                tag,
            });
        }
    };
    let superclass = decode_optional_class(reader, context, tables, NamePosition::Superclass)?;
    let enclosing = if reader.read_boolean()? {
        match decode_value(reader, context, tables, NamePosition::Value)?.as_object() {
            Some(object) => Some(object),
            _ => return Err(DecodeError::EnclosingNotAnObject),
        }
    } else {
        None
    };
    let declaring_class = decode_optional_class(reader, context, tables, NamePosition::FrameClass)?;
    let applied_clause = if reader.read_boolean()? {
        let clause = decode_applied_clause(reader, context, None)?;
        let named = decode_class_tag(reader, context, tables, NamePosition::Superclass)?;
        Some((clause, named))
    } else {
        None
    };
    // A body-less declaration is applied by no class: its class object is
    // the level its last clause adds, which applies that clause's mixin
    // (`object-model.md`, "Mixin application").
    if !mixin.has_body {
        return Err(DecodeError::BodyLessMixinApplied);
    }
    // A record only ever describes a program's class: a shipped class is a
    // shipped name, resolved against the loading world, and never an
    // object-table entry. Its metaclass is its header, which
    // `pair_metaclasses` holds to it once every record is read.
    let class = NewClass {
        application: placeholder_application.clone(),
        enclosing,
        declaring_class,
        superclass,
        builtin: None,
    };
    Ok((class, applied_clause, mixin))
}

/// A `MIXIN_SHIPPED` reference's index, after its tag: the shipped class
/// whose mixin a level applies, which must be a class -- a level applies the
/// mixin of a class, never of a metaclass, a level or a module instance --
/// that a program may name as a superclass, so neither the VM mirror's
/// class, which has no name, nor `Top` can be applied.
fn read_shipped_mixin<'gc>(
    reader: &mut Reader,
    shipped: &[ResolvedName<'gc>],
) -> Result<ClassReference<'gc>, DecodeError> {
    let entry = read_shipped_entry(reader, shipped)?;
    if entry.name.kind != ShippedKind::Class {
        return Err(DecodeError::NotAClassReference);
    }
    allowed(entry, NamePosition::Superclass)?;
    Ok(entry.object)
}

/// A `MIXIN_SHIPPED` class record, after its tag: the shipped class whose
/// mixin the level applies, its superclass and the clause that added it.
fn decode_shipped_level<'gc>(
    reader: &mut Reader,
    context: &DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
    placeholder_application: &Rc<crate::mixin::MixinApplication>,
) -> Result<DecodedClassRecord<'gc>, DecodeError> {
    let named = read_shipped_mixin(reader, context.shipped)?;
    let heap = &context.mutator.heap;
    let mixin = heap
        .class_application(named)
        .map(|application| application.mixin.clone())
        .ok_or(DecodeError::NotAClass)?;
    let superclass = decode_optional_class(reader, context, tables, NamePosition::Superclass)?;
    let applied_clause = decode_applied_clause(reader, context, heap.class_builtin(named))?;
    let named_class = decode_class_tag(reader, context, tables, NamePosition::Superclass)?;
    let class = NewClass {
        application: placeholder_application.clone(),
        enclosing: heap.class_enclosing(named),
        declaring_class: heap.class_declaring(named),
        superclass,
        builtin: None,
    };
    Ok((class, Some((applied_clause, named_class)), mixin))
}

/// The scope an `OBJECT_BODY_LESS_CLASS` record carries after its clause:
/// the declaration's enclosing object, a value that must be an object, and
/// its declaring class, a class tag; each behind a presence flag, both
/// absent for a module's root. Read as the record's own enclosing object
/// and declaring class are: a reference past the table is refused here, and
/// a declaring class that is no class object by `check_class_references`
/// once every record is read.
fn decode_declaration_scope<'gc>(
    reader: &mut Reader,
    context: &mut DecodeContext<'_, 'gc>,
    tables: &Tables<'_, 'gc>,
) -> Result<crate::heap::DeclarationScope<'gc>, DecodeError> {
    let enclosing = if reader.read_boolean()? {
        match decode_value(reader, context, tables, NamePosition::Value)?.as_object() {
            Some(object) => Some(object),
            _ => return Err(DecodeError::EnclosingNotAnObject),
        }
    } else {
        None
    };
    let declaring_class = decode_optional_class(reader, context, tables, NamePosition::FrameClass)?;
    Ok(crate::heap::DeclarationScope {
        enclosing,
        declaring_class,
    })
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
    if index as usize >= crate::mixin::CLAUSE_INDEX_LIMIT {
        return Err(DecodeError::ClauseIndexTooLarge { row, index });
    }
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
// from aborting the process. It is still refused here, because every
// reified frame has exactly its body's register file -- `Heap::reify`
// allocates the activation's whole window -- and the encoder writes all of
// it, so a frame of any other size is not something a run produces.
// ---------------------------------------------------------------------------

/// The compiled body that runs (or ran) in `frame`, or `None` when the
/// image gives it none: a `TopLevel` frame, or a class-carrying frame whose
/// kind names a body that class's mixin does not carry. Neither can be
/// entered -- nothing re-activates a decoded frame, and no closure may be
/// captured by one (`verify_closure` refuses that) -- so there is nothing to
/// reconcile.
fn frame_body<'gc>(heap: &Heap<'gc>, frame: FrameReference<'gc>) -> Option<Rc<CompiledCode>> {
    // The body the record named by code id, which `fill_frame_fields`
    // recorded as a reified frame records its activation's, and which
    // `runs_its_kind` holds to the frame's kind.
    heap.frame_code(frame)
}

/// Every decoded frame and every decoded closure against the code the
/// decoded mixins carry. First a frame's body, which its record names by
/// code id, must be one its kind runs (`runs_its_kind`), since what
/// follows means nothing against a body the frame never ran. Then the
/// four gaps a per-record decoder cannot see are closed:
///
/// 0. A frame's `program_counter` must be an instruction index of the body
///    that runs in it. A frame's `program_counter` counts instructions (an
///    image carries only bytecode), but which body it counts them in is only
///    known once the decoded mixins carry their code.
/// 1. A frame's register file must be the body's, register for register:
///    every reified frame has a field for each of its body's arguments and
///    each of its registers above the header -- none for the header's
///    numbers, which no instruction names -- and the encoder writes all of
///    them. Its arguments are its code's, so the record carries no split of
///    its own.
/// 2. A closure's `captured_frame` must be an activation of the body its
///    own body is lexically nested in. Nothing else in the format ties the
///    two together, and `verify_code_table`'s outer-local bound -- which
///    walks the *static* `MakeClosure` nesting -- is only a bound on the
///    real `frame_at_depth` walk because of this.
/// 3. A frame must run a body-less declaration's own code exactly when it
///    runs it at that declaration's class object
///    (`Heap::frame_runs_declaration_code`), since the code's mark, not the
///    frame, decides where its lexical sends start.
/// 4. A frame has a `home` exactly when its kind is a closure's, as
///    `Heap::allocate_frame` holds every frame the runtime reifies to.
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
        let kind = heap.frame_kind(*frame);
        if heap.frame_home(*frame).is_some() != matches!(kind, FrameKind::Closure(_)) {
            return Err(DecodeError::FrameHomeMismatch);
        }
        let Some(code) = frame_body(heap, *frame) else {
            // Only a top-level frame runs no body (`decode_frame`).
            if matches!(kind, FrameKind::TopLevel) {
                continue;
            }
            return Err(DecodeError::FrameCodeMismatch);
        };
        if !runs_its_kind(heap, &kind, &code) {
            return Err(DecodeError::FrameCodeMismatch);
        }
        let (program_counter, argument_count, register_count) = heap.frame_register_shape(*frame);
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
        if register_count != code.frame_register_count() {
            return Err(DecodeError::FrameRegisterFile {
                arguments: argument_count,
                locals: register_count.saturating_sub(argument_count),
                argument_count: code.argument_count,
                expected_locals: code.registers_above_header(),
                register_count: code.register_count,
            });
        }
    }
    // Every closure the graph holds is an object record -- a closure frame
    // names one of these (`check_class_references` refused any that does
    // not) -- so this one loop also covers the closure of every closure
    // activation.
    for object in &graph.objects {
        if let (Some(code), Some(captured_frame)) = (
            heap.closure_code(*object),
            heap.closure_captured_frame(*object),
        ) {
            verify_closure(heap, &code, captured_frame, table, &parents, &index_of)?;
        }
    }
    // A frame runs a body-less declaration's own code exactly when it runs
    // it at the declaration's class object: what the sends read off the
    // code (`sends::lexical_origin`) and a debug build holds to the frame.
    // Every activation a decoded frame later gives rise to -- a block it
    // created, evaluated -- takes its current class from the frame and its
    // code's mark from the frame's code, so it agrees too. After the
    // closures, whose captured frames bound the walk by the static nesting.
    for frame in &graph.frames {
        let (Some(code), Some(class)) =
            (frame_body(heap, *frame), heap.frame_current_class(*frame))
        else {
            continue;
        };
        let runs_declaration_code = heap.class_has_declaration_scope(class)
            && heap.frame_runs_declaration_code(*frame, class);
        if code.is_declaration_code() != runs_declaration_code {
            return Err(DecodeError::FrameCodeMismatch);
        }
    }
    Ok(())
}

/// Whether `code` is a body a frame of `kind` runs: a method frame a method
/// body of its method's argument count, a closure frame its closure's own
/// code, an initializer frame an initializer, a superclass clause frame a
/// superclass clause. Nothing ties a
/// decoded frame's body to its kind but this, since the image names the
/// body by code id.
fn runs_its_kind<'gc>(heap: &Heap<'gc>, kind: &FrameKind<'gc>, code: &Rc<CompiledCode>) -> bool {
    match kind {
        FrameKind::Method(method) => {
            code.kind == CodeKind::Method
                && Rc::ptr_eq(&method.code, code)
                && usize::from(code.argument_count) == method.argument_count
        }
        FrameKind::Closure(closure) => heap
            .closure_code(*closure)
            .is_some_and(|own| Rc::ptr_eq(&own, code)),
        FrameKind::Initializer(..) => code.kind == CodeKind::Initializer,
        FrameKind::Factory(_) => code.kind == CodeKind::Method,
        FrameKind::SuperclassClause(_) => code.kind == CodeKind::SuperclassClause,
        FrameKind::TopLevel => false,
    }
}

fn verify_closure<'gc>(
    heap: &Heap<'gc>,
    code: &Rc<CompiledCode>,
    captured_frame: FrameReference<'gc>,
    table: &DecodedCodeTable,
    parents: &[Option<usize>],
    index_of: &HashMap<usize, usize>,
) -> Result<(), DecodeError> {
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
    let running = frame_body(heap, captured_frame).ok_or(DecodeError::CapturedFrameMismatch)?;
    if !Rc::ptr_eq(&running, &table.codes[parent]) {
        return Err(DecodeError::CapturedFrameMismatch);
    }
    Ok(())
}
