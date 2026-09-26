//! The image's mixin table: one row per class declaration, flat.
//!
//! A `Mixin` is what a loaded image has to reconstruct before anything can
//! run -- the method dictionary a send is looked up in, the slot names an
//! instance is laid out by, the factory selector §6.1 checks a superclass
//! clause against. `image::load` builds its whole declaration tree, and its
//! root class, from these rows, which is what keeps the front end off the
//! load path: without them those would come back only by re-parsing the
//! image's own source text, and the image's own bytes would be an
//! incomplete description of the world they claim to hold.
//!
//! **The tree is flattened, not nested.** Rows are written in
//! `mixin::preorder` order -- the same order `encode::encode_code_table`
//! collects the bodies in, so a row's code ids follow the rows -- and a row
//! names its nested declarations and its class side by row id, and each of
//! its bodies by code-table id. Nothing in the byte
//! stream is nested inside anything else, so decoding is a loop over rows
//! and never a recursion on a depth the image controls. The one structural
//! recursion left in the runtime (`mixin::preorder`) is protected by
//! `MAX_MIXIN_DEPTH` below, which is also what keeps dropping a decoded tree
//! -- itself a recursive walk of `Rc<Mixin>` children -- inside the stack.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast::AccessModifier;
use ns_syntax::Span;

use crate::bytecode::CompiledCode;
use crate::heap::Heap;
use crate::mixin::{
    mixin_key, slot_accessors, Factory, MethodEntry, MethodFastPath, Mixin, MixinApplicationClause,
    MixinMethod, MixinSlot, SuperclassClause,
};
use crate::types::ClassReference;
use crate::Symbol;

use super::decode::{reserve_for, DecodeError, Reader};
use super::encode::{u32_field, CodeIds, Writer};
use super::opcodes::{SLOT_IMMUTABLE, SLOT_LAZY_IMMUTABLE, SLOT_LAZY_MUTABLE, SLOT_MUTABLE};
use super::symbols;

/// `ns-syntax`'s own nesting ceiling: the parser refuses a module whose
/// classes nest deeper than this (`MAX_DEPTH` in
/// `crates/ns-syntax/src/parser/mod.rs`, which is private to that crate, so
/// the number has to be restated rather than imported).
///
/// It counts *nested* classes only: `parser/declaration.rs` calls `Parser::enter`
/// where a class body starts a nested class declaration, and nowhere for the
/// module's root class.
const PARSER_MAX_NESTED_CLASS_DEPTH: usize = 128;

/// How deep a declaration tree may nest, counted in `Rc<Mixin>` links from
/// the root -- the depth `check_preorder` measures on the way in and `plan`
/// measures on the way out.
///
/// The depth is attacker-controlled, and three separate walks over a decoded
/// tree recurse on it: `mixin::preorder`, `Rc<Mixin>`'s own drop glue, and
/// `Debug`. Decoding itself is iterative, so this ceiling is not what keeps
/// the *decoder* off the stack -- it is what keeps everything downstream of
/// a successful decode off it.
///
/// Derived from the front end's ceiling rather than chosen independently: a
/// limit below what the parser accepts would refuse to round-trip a program
/// the runtime already runs happily from source, and this project does not
/// exclude a valid construct for being unusual. The two extra levels are
/// exactly the difference between what the parser counts and what a `Mixin`
/// tree links:
///
/// - the parser does not count the root class, so the deepest chain it
///   accepts is `PARSER_MAX_NESTED_CLASS_DEPTH` nested classes *under* a
///   root -- one more `Mixin` than the parser's number;
/// - `Mixin::class_side` is a child link in the tree but not a parser
///   nesting level, and the innermost class of that chain may have one --
///   one more level again, at the bottom.
///
/// `the_deepest_module_the_parser_accepts_round_trips` in
/// `crates/ns-runtime/tests/image.rs` pins the relationship from the other
/// end: it discovers the parser's real limit by probing rather than
/// restating it, builds the deepest module that limit allows, and requires
/// it to snapshot and load. If the parser's ceiling moves, that test fails
/// here.
const MAX_MIXIN_DEPTH: usize = PARSER_MAX_NESTED_CLASS_DEPTH + 2;

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

/// One declaration's row, with its children already resolved to row ids.
/// Its code is read off `mixin` and `methods` themselves, which carry it.
pub struct Row {
    pub mixin: Rc<Mixin>,
    pub nested: Vec<u32>,
    pub class_side: Option<u32>,
    /// The compiled methods, sorted by selector text: the mixin's method
    /// table is a `HashMap`, and sorting is what makes the row canonical
    /// rather than dependent on that map's iteration order. The table's slot
    /// accessors are not listed: they are derived from the slots
    /// (`mixin::slot_accessors`), on the way in as on the way out.
    pub methods: Vec<Rc<MixinMethod>>,
}

/// Flattens the tree under `root_class` into rows, in `mixin::preorder`
/// order.
///
/// Called by both `encode::encode_code_table` (which needs the rows to know
/// which `CompiledCode` is reachable, and in what order) and `encode` below
/// (which needs them to write the table). Recomputing it is one walk of the
/// tree and one `HashMap`; sharing a cached copy between the two would trade
/// that for a lifetime that has to thread through `snapshot`.
pub fn plan<'gc>(heap: &Heap<'gc>, root_class: ClassReference<'gc>) -> Result<Vec<Row>, String> {
    let root_mixin = heap
        .class_application(root_class)
        .map(|application| application.mixin.clone());
    // Not a class object: no declarations to describe, so the table is
    // empty rather than an error -- `encode_code_table` has always tolerated
    // this, and the graph section is what actually refuses such a root.
    let Some(root_mixin) = root_mixin else {
        return Ok(Vec::new());
    };
    let mut order = Vec::new();
    crate::mixin::preorder(&root_mixin, &mut order);
    let ids: HashMap<usize, u32> = order
        .iter()
        .enumerate()
        .map(|(index, mixin)| (mixin_key(mixin), u32_field("mixin table id", index)))
        .collect();
    let mut rows = Vec::with_capacity(order.len());
    for mixin in &order {
        let mut methods: Vec<Rc<MixinMethod>> = mixin.compiled_methods().cloned().collect();
        methods.sort_by(|left, right| left.selector.as_str().cmp(right.selector.as_str()));
        rows.push(Row {
            mixin: mixin.clone(),
            // Every child is in `order` -- `preorder` put it there -- so the
            // lookup cannot miss.
            nested: mixin
                .nested
                .iter()
                .map(|child| ids[&mixin_key(child)])
                .collect(),
            class_side: mixin.class_side.as_ref().map(|side| ids[&mixin_key(side)]),
            methods,
        });
    }
    // Refused here, not left for the reader: an image nested past the
    // decoder's ceiling would write cleanly and then be unloadable
    // everywhere, turning a local mistake into someone else's failure.
    // Unreachable for a tree built from parsed source (the ceiling is
    // derived from the parser's own), so what this catches is a mixin tree
    // assembled programmatically.
    let depth = depth_of(&rows);
    if depth > MAX_MIXIN_DEPTH {
        return Err(format!(
            "image encoding refuses a class tree nested {depth} levels deep: \
             the mixin table's ceiling is {MAX_MIXIN_DEPTH} levels, and an \
             image past it could never be loaded"
        ));
    }
    Ok(rows)
}

/// The tree's depth in `Mixin` links, counting the root as level 1.
///
/// One reverse pass: `plan` emits rows in `mixin::preorder`, so every
/// child's row id is greater than its parent's and a row's children are
/// already measured when it is reached. Iterative for the same reason the
/// decoder's walk is -- and, unlike the decoder's, over a tree this process
/// built, so the loop is here to share one definition of "depth" with
/// `check_preorder` rather than to survive hostile input.
fn depth_of(rows: &[Row]) -> usize {
    let mut depth = vec![0usize; rows.len()];
    for (index, row) in rows.iter().enumerate().rev() {
        let deepest_child = row
            .nested
            .iter()
            .chain(row.class_side.iter())
            .map(|id| depth[*id as usize])
            .max()
            .unwrap_or(0);
        depth[index] = deepest_child + 1;
    }
    depth.into_iter().max().unwrap_or(0)
}

/// Explicit wire tags for `AccessModifier` -- never `as u8` on the enum,
/// whose declaration order is not a wire format.
pub fn access_tag(access: AccessModifier) -> u8 {
    match access {
        AccessModifier::Public => 0,
        AccessModifier::Protected => 1,
        AccessModifier::Private => 2,
    }
}

/// A slot record's kind tag (`opcodes::SLOT_*`): the accessor shape the
/// slot declares -- whether it has a setter (§6.3.2: only a mutable slot
/// does) and whether its getter is a lazy slot's compiled method (§6.3.3).
pub fn slot_kind_tag(mutable: bool, lazy: bool) -> u8 {
    match (mutable, lazy) {
        (false, false) => SLOT_IMMUTABLE,
        (true, false) => SLOT_MUTABLE,
        (false, true) => SLOT_LAZY_IMMUTABLE,
        (true, true) => SLOT_LAZY_MUTABLE,
    }
}

/// `slot_kind_tag` read back, as `(mutable, lazy)`.
fn slot_kind_from_tag(tag: u8) -> Result<(bool, bool), DecodeError> {
    match tag {
        SLOT_IMMUTABLE => Ok((false, false)),
        SLOT_MUTABLE => Ok((true, false)),
        SLOT_LAZY_IMMUTABLE => Ok((false, true)),
        SLOT_LAZY_MUTABLE => Ok((true, true)),
        tag => Err(DecodeError::InvalidTag {
            what: "slot kind",
            tag,
        }),
    }
}

fn access_from_tag(tag: u8) -> Result<AccessModifier, DecodeError> {
    match tag {
        0 => Ok(AccessModifier::Public),
        1 => Ok(AccessModifier::Protected),
        2 => Ok(AccessModifier::Private),
        tag => Err(DecodeError::InvalidTag {
            what: "access modifier",
            tag,
        }),
    }
}

/// `mixin_key` -> mixin-table row id, as `encode` assigns them. Named so
/// `image::mod`'s `Describing` struct can hold one without spelling out the
/// map it happens to be today.
pub(crate) type MixinIds = HashMap<usize, u32>;

/// Writes the mixin table and returns the `mixin_key` -> row id map the
/// object graph addresses a class object's mixin by.
pub fn encode<'gc>(
    writer: &mut Writer,
    heap: &Heap<'gc>,
    root_class: ClassReference<'gc>,
    symbols: &mut symbols::Builder,
    codes: &CodeIds,
) -> Result<MixinIds, String> {
    let rows = plan(heap, root_class)?;
    writer.write_u32(u32_field("mixin table entry count", rows.len()));
    for row in &rows {
        let mixin = &row.mixin;
        writer.write_u32(symbols.intern(mixin.name.as_str()));
        writer.write_u8(access_tag(mixin.access));
        writer.write_u32(mixin.span.start);
        writer.write_u32(mixin.span.end);
        writer.write_u32(u32_field("mixin's slot count", mixin.slots.len()));
        // A slot's name, kind and accessibility decide the accessors it
        // defines (§6.3.2), which the decoder derives from them -- all but a
        // lazy slot's getter, which is one of the row's methods (§6.3.3).
        for slot in &mixin.slots {
            writer.write_u32(symbols.intern(slot.name.as_str()));
            writer.write_u8(slot_kind_tag(slot.mutable, slot.lazy));
            writer.write_u8(access_tag(slot.access));
        }
        match &mixin.factory {
            Some(factory) => {
                writer.write_boolean(true);
                writer.write_u32(symbols.intern(factory.selector.as_str()));
                writer.write_u16(u16_arity("factory", factory.argument_count));
            }
            None => writer.write_boolean(false),
        }
        writer.write_boolean(mixin.has_initializer_expressions);
        match &mixin.superclass {
            Some(clause) => {
                writer.write_boolean(true);
                writer.write_u32(symbols.intern(clause.selector.as_str()));
                writer.write_boolean(clause.has_arguments);
            }
            None => writer.write_boolean(false),
        }
        // §6.2's `<:` clauses, in declaration order. How each one's receiver
        // is identified is not here: it is evaluated by the superclass
        // clause's body, which answers the superclass and every named class
        // together (`interpreter::nested::superclass_clause_expression`) --
        // so the count here is also how many classes that body answers
        // beside the superclass.
        writer.write_u32(u32_field(
            "mixin's mixin application count",
            mixin.mixin_applications.len(),
        ));
        for clause in &mixin.mixin_applications {
            writer.write_u32(symbols.intern(clause.selector.as_str()));
            writer.write_boolean(clause.has_arguments);
            write_optional_id(
                writer,
                clause.message_code.as_ref().map(|code| codes.id_of(code)),
            );
        }
        writer.write_u32(u32_field("mixin's method count", row.methods.len()));
        for method in &row.methods {
            writer.write_u32(symbols.intern(method.selector.as_str()));
            writer.write_u8(access_tag(method.access));
            writer.write_u16(u16_arity("method", method.argument_count));
            writer.write_u32(codes.id_of(&method.code));
        }
        writer.write_u32(u32_field("mixin's nested count", row.nested.len()));
        for id in &row.nested {
            writer.write_u32(*id);
        }
        write_optional_id(writer, row.class_side);
        write_optional_id(
            writer,
            mixin
                .initializer_code
                .as_ref()
                .map(|code| codes.id_of(code)),
        );
        write_optional_id(
            writer,
            mixin
                .superclass_clause_code
                .as_ref()
                .map(|code| codes.id_of(code)),
        );
        write_optional_id(
            writer,
            mixin
                .superclass_message_code
                .as_ref()
                .map(|code| codes.id_of(code)),
        );
    }
    Ok(rows
        .iter()
        .enumerate()
        .map(|(index, row)| (mixin_key(&row.mixin), u32_field("mixin table id", index)))
        .collect())
}

/// A parameter count narrowed to the `u16` the wire (and `CompiledCode`)
/// stores an arity in. A pattern with more than 65535 keyword parts is not
/// something any parser produces; writing a truncated one would make the
/// image self-inconsistent, so it aborts rather than truncating silently --
/// the same contract as `encode::u32_field`.
fn u16_arity(what: &str, count: usize) -> u16 {
    assert!(
        u16::try_from(count).is_ok(),
        "image {what} arity of {count} does not fit the format's u16 field"
    );
    count as u16
}

fn write_optional_id(writer: &mut Writer, id: Option<u32>) {
    match id {
        Some(id) => {
            writer.write_boolean(true);
            writer.write_u32(id);
        }
        None => writer.write_boolean(false),
    }
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// One decoded row's *code* half as the row said it: the code-table id of
/// each of the mixin's members' bodies. The bodies themselves are on the
/// decoded mixin; the ids are kept beside it, because a code-table id is a
/// fact about one image rather than about a declaration, and `inspect`
/// attributes each code-table entry to its owner by them.
#[derive(Debug, PartialEq)]
pub struct MixinCode {
    /// `(selector, code id)`, sorted by selector exactly as encoded.
    pub methods: Vec<(Symbol, u32)>,
    pub initializer: Option<u32>,
    pub superclass_clause: Option<u32>,
    pub superclass_message: Option<u32>,
    /// Each `<:` clause's message body, in clause order.
    pub mixin_applications: Vec<Option<u32>>,
}

/// The decoded mixin table: the rebuilt declarations, in row-id order (which
/// is `mixin::preorder` order of `mixins[0]`), and each one's code bindings
/// at the same index.
#[derive(Debug)]
pub struct Decoded {
    pub mixins: Vec<Rc<Mixin>>,
    pub code: Vec<MixinCode>,
}

impl Decoded {
    pub fn empty() -> Decoded {
        Decoded {
            mixins: Vec::new(),
            code: Vec::new(),
        }
    }

    /// The tree's root -- row 0, which `decode` has already established
    /// every other row hangs under. `None` for an empty table.
    pub fn root(&self) -> Option<&Rc<Mixin>> {
        self.mixins.first()
    }
}

/// A row as read, before its children exist. Split out because a row names
/// children by id and the ids are only known to be sound once every row has
/// been read.
struct RawRow {
    name: Symbol,
    access: AccessModifier,
    span: Span,
    slots: Vec<RawSlot>,
    factory: Option<Factory>,
    has_initializer_expressions: bool,
    superclass: Option<SuperclassClause>,
    mixin_applications: Vec<RawClause>,
    methods: Vec<(Symbol, AccessModifier, usize, u32)>,
    nested: Vec<u32>,
    class_side: Option<u32>,
    initializer: Option<u32>,
    superclass_clause: Option<u32>,
    superclass_message: Option<u32>,
}

/// A `<:` clause as read: its selector, whether its message takes arguments,
/// and the code id of their body.
struct RawClause {
    selector: Symbol,
    has_arguments: bool,
    message: Option<u32>,
}

/// A slot as read: its name, kind and accessibility.
struct RawSlot {
    name: Symbol,
    mutable: bool,
    lazy: bool,
    access: AccessModifier,
}

/// Reads the mixin table.
///
/// `codes` is the already-decoded code table: every code id a row names is
/// range-checked against it here, at the point of use, so no later pass can
/// index it unchecked.
///
/// Three passes, all iterative:
///
/// 1. read every row, validating each count against the remaining input and
///    each symbol and code id against its table;
/// 2. verify that the rows are exactly `mixin::preorder` of row 0 -- one
///    explicit-stack walk that simultaneously establishes the rows form a
///    tree (no row is its own ancestor, no row has two parents, no row is
///    unreachable) and that the tree is not deeper than `MAX_MIXIN_DEPTH`;
/// 3. build the `Rc<Mixin>`s from the last row to the first. Step 2 proved
///    every child's id is greater than its parent's, so a row's children are
///    always already built when it is reached, and no back-patching pass or
///    interior mutability is needed to close the tree.
pub fn decode(
    reader: &mut Reader,
    symbols: &symbols::Table,
    codes: &[Rc<CompiledCode>],
) -> Result<Decoded, DecodeError> {
    let count = reader.read_u32()? as usize;
    let mut raw: Vec<RawRow> = reserve_for(count, reader);
    for _ in 0..count {
        raw.push(read_row(reader, symbols, codes)?);
    }
    if raw.is_empty() {
        return Ok(Decoded::empty());
    }
    check_ids(&raw)?;
    check_preorder(&raw)?;
    check_member_names(&raw)?;
    check_lazy_slot_getters(&raw)?;
    Ok(build(raw, codes))
}

fn read_row(
    reader: &mut Reader,
    symbols: &symbols::Table,
    codes: &[Rc<CompiledCode>],
) -> Result<RawRow, DecodeError> {
    let name = read_symbol(reader, symbols)?;
    let access = access_from_tag(reader.read_u8()?)?;
    let span = read_span(reader)?;
    let slot_count = reader.read_u32()? as usize;
    let mut slots: Vec<RawSlot> = reserve_for(slot_count, reader);
    for _ in 0..slot_count {
        let name = read_symbol(reader, symbols)?;
        let (mutable, lazy) = slot_kind_from_tag(reader.read_u8()?)?;
        slots.push(RawSlot {
            name,
            mutable,
            lazy,
            access: access_from_tag(reader.read_u8()?)?,
        });
    }
    let factory = if reader.read_boolean()? {
        Some(Factory {
            selector: read_symbol(reader, symbols)?,
            argument_count: reader.read_u16()? as usize,
        })
    } else {
        None
    };
    let has_initializer_expressions = reader.read_boolean()?;
    let superclass = if reader.read_boolean()? {
        Some(SuperclassClause {
            selector: read_symbol(reader, symbols)?,
            has_arguments: reader.read_boolean()?,
        })
    } else {
        None
    };
    let clause_count = reader.read_u32()? as usize;
    let mut mixin_applications: Vec<RawClause> = reserve_for(clause_count, reader);
    for _ in 0..clause_count {
        mixin_applications.push(RawClause {
            selector: read_symbol(reader, symbols)?,
            has_arguments: reader.read_boolean()?,
            message: read_optional(reader, |reader| read_code_id(reader, codes))?,
        });
    }
    let method_count = reader.read_u32()? as usize;
    let mut methods: Vec<(Symbol, AccessModifier, usize, u32)> = reserve_for(method_count, reader);
    for _ in 0..method_count {
        let selector = read_symbol(reader, symbols)?;
        let access = access_from_tag(reader.read_u8()?)?;
        let argument_count = reader.read_u16()? as usize;
        let code = read_code_id(reader, codes)?;
        methods.push((selector, access, argument_count, code));
    }
    let nested_count = reader.read_u32()? as usize;
    let mut nested: Vec<u32> = reserve_for(nested_count, reader);
    for _ in 0..nested_count {
        nested.push(reader.read_u32()?);
    }
    let class_side = read_optional(reader, |reader| reader.read_u32())?;
    let initializer = read_optional(reader, |reader| read_code_id(reader, codes))?;
    let superclass_clause = read_optional(reader, |reader| read_code_id(reader, codes))?;
    let superclass_message = read_optional(reader, |reader| read_code_id(reader, codes))?;
    Ok(RawRow {
        name,
        access,
        span,
        slots,
        factory,
        has_initializer_expressions,
        superclass,
        mixin_applications,
        methods,
        nested,
        class_side,
        initializer,
        superclass_clause,
        superclass_message,
    })
}

fn read_symbol(reader: &mut Reader, symbols: &symbols::Table) -> Result<Symbol, DecodeError> {
    symbols.get(reader.read_u32()?)
}

fn read_span(reader: &mut Reader) -> Result<Span, DecodeError> {
    let start = reader.read_u32()?;
    let end = reader.read_u32()?;
    if start > end {
        return Err(DecodeError::InvalidSpan { start, end });
    }
    Ok(Span::new(start, end))
}

fn read_code_id(reader: &mut Reader, codes: &[Rc<CompiledCode>]) -> Result<u32, DecodeError> {
    let id = reader.read_u32()?;
    if (id as usize) < codes.len() {
        Ok(id)
    } else {
        Err(DecodeError::BadCodeId(id))
    }
}

fn read_optional<T>(
    reader: &mut Reader,
    read: impl FnOnce(&mut Reader) -> Result<T, DecodeError>,
) -> Result<Option<T>, DecodeError> {
    if reader.read_boolean()? {
        Ok(Some(read(reader)?))
    } else {
        Ok(None)
    }
}

/// Every child id inside the table. Checked before the structural walk so
/// that the walk itself can index `raw` without a bounds test per step.
fn check_ids(raw: &[RawRow]) -> Result<(), DecodeError> {
    for row in raw {
        for id in row.nested.iter().chain(row.class_side.iter()) {
            if (*id as usize) >= raw.len() {
                return Err(DecodeError::BadMixinId(*id));
            }
        }
    }
    Ok(())
}

/// No row declares one member twice -- the rule the resolver applies to
/// source (`resolver::classes::duplicate_member_names`), over the members a
/// row declares: its methods, its slots' accessors (a getter under the
/// slot's name, a mutable slot's setter under the name with `:`, §6.3.2),
/// its nested classes' accessors (§6) and, for a row that is some
/// declaration's class side, the primary factory §6.3 puts on the metaclass
/// mixin. A row that broke it would build a method table that silently keeps
/// one of two members: a nested class no send can reach, or a class that no
/// longer instantiates because a class-side method took the factory's
/// selector. The encoder writes only what the resolver accepted, so this is
/// only reachable from a crafted image.
///
/// Runs after `check_ids`, which is what lets it read a nested row's name.
fn check_member_names(raw: &[RawRow]) -> Result<(), DecodeError> {
    let factory_entries = metaclass_factory_entries(raw);
    for (index, row) in raw.iter().enumerate() {
        let factory = factory_entries[index]
            .as_ref()
            .map(|(selector, _)| *selector)
            .into_iter();
        let setters = row
            .slots
            .iter()
            .filter(|slot| slot.mutable)
            .map(|slot| Symbol::intern(&format!("{}:", slot.name)));
        // A lazy slot's getter is one of the row's methods, so its name is
        // counted there, not among the members below. Two lazy slots of one
        // name would then share that one getter, which serves only one of
        // their fields and leaves the other silently dead, so they are
        // refused here as the duplicate getter they are.
        let mut lazy_names = std::collections::HashSet::new();
        for slot in row.slots.iter().filter(|slot| slot.lazy) {
            if !lazy_names.insert(slot.name) {
                return Err(DecodeError::DuplicateSelector(
                    slot.name.as_str().to_string(),
                ));
            }
        }
        let members = row
            .slots
            .iter()
            .filter(|slot| !slot.lazy)
            .map(|slot| slot.name)
            .chain(setters)
            .chain(row.methods.iter().map(|&(selector, ..)| selector))
            .chain(row.nested.iter().map(|id| raw[*id as usize].name))
            .chain(factory);
        let mut seen = std::collections::HashSet::new();
        for member in members {
            if !seen.insert(member) {
                return Err(DecodeError::DuplicateSelector(member.as_str().to_string()));
            }
        }
    }
    Ok(())
}

/// Every lazy slot a row declares (§6.3.3) has its getter among the row's
/// methods: a method of the slot's name that takes no argument and has the
/// slot's accessibility, as `MixinMethod::lazy_slot_getter` built it. The
/// getter is not derived from the slot record the way an accessor is -- it
/// carries the initializer's compiled body -- so a row without one would
/// decode into a slot no send can read, and a row whose method of that name
/// takes an argument would give `build` a `LazySlot` fast path for a method
/// that is not a getter. Runs after `check_member_names`, so the method is
/// the only member of that name.
///
/// Through a map rather than a scan of the methods per slot, because both
/// counts are the image's to choose: a scan would make the check quadratic
/// in the input.
fn check_lazy_slot_getters(raw: &[RawRow]) -> Result<(), DecodeError> {
    for row in raw {
        if !row.slots.iter().any(|slot| slot.lazy) {
            continue;
        }
        let methods: HashMap<Symbol, (AccessModifier, usize)> = row
            .methods
            .iter()
            .map(|&(selector, access, argument_count, _)| (selector, (access, argument_count)))
            .collect();
        for slot in row.slots.iter().filter(|slot| slot.lazy) {
            if methods.get(&slot.name) != Some(&(slot.access, 0)) {
                return Err(DecodeError::LazySlotWithoutGetter(slot.name.to_string()));
            }
        }
    }
    Ok(())
}

/// The rows must be exactly `mixin::preorder(row 0)`: row `i` is the `i`th
/// declaration that walk reaches.
///
/// Row position is how a class object's record names the declaration it
/// applies (`decode::decode_class_record` indexes the rows by id), so this
/// is the property that makes a row's position mean anything at all. It is
/// also, as a by-product, the whole
/// structural sanity check: an index is accepted only when it equals the
/// step counter, so every row is visited exactly once (no shared child, no
/// cycle -- a cycle would revisit an index already behind the counter) and
/// the walk ends having visited all of them (no unreachable row, no second
/// root). Iterative, over an explicit stack, because the depth is the
/// image's to choose.
fn check_preorder(raw: &[RawRow]) -> Result<(), DecodeError> {
    let mut stack: Vec<(u32, usize)> = vec![(0, 1)];
    let mut expected: u32 = 0;
    while let Some((id, depth)) = stack.pop() {
        if id != expected {
            return Err(DecodeError::MixinTableNotATree { row: id });
        }
        if depth > MAX_MIXIN_DEPTH {
            return Err(DecodeError::NestingTooDeep {
                what: "mixin nesting",
                limit: MAX_MIXIN_DEPTH,
            });
        }
        expected += 1;
        let row = &raw[id as usize];
        // `preorder` visits the nested declarations first and the class
        // side last, so they go onto the stack in the opposite order.
        if let Some(class_side) = row.class_side {
            stack.push((class_side, depth + 1));
        }
        for id in row.nested.iter().rev() {
            stack.push((*id, depth + 1));
        }
    }
    if (expected as usize) != raw.len() {
        return Err(DecodeError::MixinTableNotATree { row: expected });
    }
    Ok(())
}

/// The factory entry each row holds as a *metaclass* mixin: for a row some
/// other row names as its class side, the entry that other row's own factory
/// defines (`newspeak-spec.pdf` §6.3, "The class side, together with the
/// primary factory method, defines the mixin for the metaclass"); `None` for
/// every other row, which is not a metaclass mixin and must not answer a
/// factory selector.
///
/// The entry is derived rather than written to the image, exactly as a
/// slot's accessors are (§6.3.2): it carries no body and everything it needs
/// is already in the parent row's factory field, so writing it would be a
/// second copy of the format's own answer, free to disagree with it.
///
/// `check_preorder` has established that every row is reached exactly once,
/// so no row is the class side of two declarations and the answer is unique.
/// Runs after `check_ids`, which is what lets this index `raw`.
fn metaclass_factory_entries(raw: &[RawRow]) -> Vec<Option<(Symbol, MethodEntry)>> {
    let mut entries: Vec<Option<(Symbol, MethodEntry)>> = vec![None; raw.len()];
    for row in raw {
        if let Some(class_side) = row.class_side {
            entries[class_side as usize] = Some(crate::mixin::factory_entry(row.factory));
        }
    }
    entries
}

/// Builds the `Rc<Mixin>`s back to front. `check_preorder` established that
/// every child's row id is greater than its parent's, so a row's children
/// are already built by the time the loop reaches it.
///
/// Each method and each per-mixin body is handed the code-table entry its
/// row names -- `read_row` range-checked every id against `codes` already.
/// Nothing here reconciles a body with what it is attached to:
/// `decode::verify_mixins` does, on the load path only, so `inspect` can
/// still display an artifact whose halves disagree.
fn build(raw: Vec<RawRow>, codes: &[Rc<CompiledCode>]) -> Decoded {
    let code_at = |id: u32| codes[id as usize].clone();
    let mut factory_entries = metaclass_factory_entries(&raw);
    let mut built: Vec<Option<Rc<Mixin>>> = vec![None; raw.len()];
    let mut code: Vec<Option<MixinCode>> = (0..raw.len()).map(|_| None).collect();
    for (index, row) in raw.into_iter().enumerate().rev() {
        let slots: Vec<MixinSlot> = row
            .slots
            .iter()
            .map(|slot| MixinSlot {
                name: slot.name,
                mutable: slot.mutable,
                access: slot.access,
                lazy: slot.lazy,
                declaration: None,
            })
            .collect();
        // `check_member_names` proved no compiled method shares a selector
        // with an accessor, so neither overwrites the other.
        let accessors = slot_accessors(&slots);
        // Each lazy slot's name and its position among the row's own slots,
        // the `own_index` its getter's fast path reads the field at.
        let lazy_slot_positions: HashMap<Symbol, u32> = slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.lazy)
            .map(|(position, slot)| {
                (
                    slot.name,
                    u32::try_from(position).expect("a slot count read as a u32 fits a u32"),
                )
            })
            .collect();
        // §6.3: a row another row names as its class side is a metaclass
        // mixin, and carries that declaration's primary factory as an entry
        // of its method table. `check_member_names` proved the row declares
        // nothing under that selector, so nothing is overwritten.
        let factory_entry = factory_entries[index].take();
        let compiled = row
            .methods
            .iter()
            .map(|&(selector, access, argument_count, id)| {
                (
                    selector,
                    MethodEntry::Compiled(Rc::new(MixinMethod {
                        selector,
                        access,
                        argument_count,
                        // No declaration to point at, and none invented:
                        // the compiled body the row names is what this
                        // method runs, and nothing reads a declaration at
                        // run time.
                        declaration: None,
                        code: code_at(id),
                        // An image never carries a kernel method: the
                        // kernel is compiled from `Kernel.ns` at bootstrap.
                        // The one fast path a decoded method takes is a
                        // lazy slot's getter's, restored from the slot
                        // record -- `check_lazy_slot_getters` proved the
                        // method of the slot's name is its getter.
                        fast_path: lazy_slot_positions
                            .get(&selector)
                            .map(|&own_index| MethodFastPath::LazySlot { own_index }),
                    })),
                )
            });
        let methods = accessors
            .into_iter()
            .chain(compiled)
            .chain(factory_entry)
            .collect();
        code[index] = Some(MixinCode {
            methods: row
                .methods
                .iter()
                .map(|&(selector, _, _, id)| (selector, id))
                .collect(),
            initializer: row.initializer,
            superclass_clause: row.superclass_clause,
            superclass_message: row.superclass_message,
            mixin_applications: row
                .mixin_applications
                .iter()
                .map(|clause| clause.message)
                .collect(),
        });
        let nested = row
            .nested
            .iter()
            .map(|id| {
                built[*id as usize]
                    .clone()
                    .expect("check_preorder proved a child's row id is greater than its own")
            })
            .collect();
        let class_side = row.class_side.map(|id| {
            built[id as usize]
                .clone()
                .expect("check_preorder proved a class side's row id is greater than its own")
        });
        let mixin = Rc::new(Mixin {
            name: row.name,
            access: row.access,
            methods,
            slots,
            factory: row.factory,
            superclass: row.superclass,
            mixin_applications: row
                .mixin_applications
                .iter()
                .map(|clause| MixinApplicationClause {
                    selector: clause.selector,
                    has_arguments: clause.has_arguments,
                    message_code: clause.message.map(code_at),
                })
                .collect(),
            has_initializer_expressions: row.has_initializer_expressions,
            initializer_expressions: Vec::new(),
            nested,
            class_side,
            span: row.span,
            declaration: None,
            initializer_code: row.initializer.map(code_at),
            superclass_clause_code: row.superclass_clause.map(code_at),
            superclass_message_code: row.superclass_message.map(code_at),
            // The decoder refuses a root section the root rule forbids
            // (`decode::decode_root_section`), so no decoded mixin has one.
            root_clause_defect: None,
        });
        built[index] = Some(mixin);
    }
    Decoded {
        mixins: built
            .into_iter()
            .map(|mixin| mixin.expect("every row was built"))
            .collect(),
        code: code
            .into_iter()
            .map(|code| code.expect("every row was built"))
            .collect(),
    }
}
