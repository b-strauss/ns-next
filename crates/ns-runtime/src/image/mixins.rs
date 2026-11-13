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
use crate::heap::{Heap, MixinId};
use crate::mixin::{
    mixin_key, slot_accessors, ClauseMessage, DeclarationText, Factory, MethodEntry, Mixin,
    MixinApplicationClause, MixinMethod, MixinSlot,
};
use crate::Symbol;

use super::decode::{check_argument_count, reserve_for, DecodeError, Reader};
use super::encode::{arity_field, u32_field, CodeIds, Writer};
use super::opcodes::{SLOT_IMMUTABLE, SLOT_LAZY_IMMUTABLE, SLOT_LAZY_MUTABLE, SLOT_MUTABLE};
use super::symbols;
use super::texts::TextIds;

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
    /// The declaration's current version, whose code, layout and text the
    /// row describes.
    pub mixin: Rc<Mixin>,
    /// The id the heap registered the declaration under, by which every
    /// record names the row (`MixinIds`); `None` for a declaration no class
    /// object applied and nothing reified, which no record can name.
    pub id: Option<MixinId>,
    pub nested: Vec<u32>,
    pub class_side: Option<u32>,
    /// The compiled methods, sorted by selector text: the mixin's method
    /// table is a `HashMap`, and sorting is what makes the row canonical
    /// rather than dependent on that map's iteration order. The table's slot
    /// accessors are not listed: they are derived from the slots
    /// (`mixin::slot_accessors`), on the way in as on the way out.
    pub methods: Vec<Rc<MixinMethod>>,
}

/// `mixin`'s current version: an install replaced it under its id when
/// one names it, and a mixin no class object applied has none.
fn current(heap: &Heap<'_>, mixin: &Rc<Mixin>) -> Rc<Mixin> {
    heap.known_mixin_id(mixin)
        .and_then(|id| heap.mixin(id))
        .unwrap_or_else(|| Rc::clone(mixin))
}

/// `mixin::preorder` over the current versions: a version's `class_side`
/// still holds the version it was built with, which an edit may since
/// have replaced under its id, and a registered version's nested
/// declarations are the heap's (`Heap::nested_mixins`).
fn current_preorder(heap: &Heap<'_>, mixin: &Rc<Mixin>, out: &mut Vec<Rc<Mixin>>) {
    let mixin = current(heap, mixin);
    out.push(Rc::clone(&mixin));
    for nested in heap.nested_mixins(&mixin) {
        current_preorder(heap, &nested, out);
    }
    if let Some(class_side) = &mixin.class_side {
        current_preorder(heap, class_side, out);
    }
}

/// Flattens the tree under each of `module_roots` into rows, in
/// `mixin::preorder` order over each declaration's current version, one
/// tree after another in module order. Row ids and the depth ceiling cover
/// the whole forest: a class object's record names its declaration by row
/// id whichever module declared it.
///
/// A row describes the current version, never the one a reference reached
/// it through, so an edited program is written as it now is: an install
/// replaces a mixin under its id while a class object, an enclosing
/// declaration or an applied clause may still name the version before.
///
/// Planned once per image and handed to every table it describes -- the
/// code table, the text table and the mixin table -- so that all three
/// follow the same rows.
pub fn plan(heap: &Heap<'_>, module_roots: &[Rc<Mixin>]) -> Result<Vec<Row>, String> {
    let mut order = Vec::new();
    for root in module_roots {
        current_preorder(heap, root, &mut order);
    }
    let rows_by_key: HashMap<usize, u32> = order
        .iter()
        .enumerate()
        .map(|(index, mixin)| (mixin_key(mixin), u32_field("mixin table id", index)))
        .collect();
    // Every child's current version is in `order` -- `current_preorder`
    // put it there -- so the lookup cannot miss.
    let row_of = |child: &Rc<Mixin>| rows_by_key[&mixin_key(&current(heap, child))];
    let mut rows = Vec::with_capacity(order.len());
    for mixin in &order {
        let mut methods: Vec<Rc<MixinMethod>> = mixin.compiled_methods().cloned().collect();
        methods.sort_by(|left, right| left.selector.as_str().cmp(right.selector.as_str()));
        rows.push(Row {
            mixin: mixin.clone(),
            id: heap.known_mixin_id(mixin),
            nested: heap.nested_mixins(mixin).iter().map(row_of).collect(),
            class_side: mixin.class_side.as_ref().map(row_of),
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

/// The forest's depth in `Mixin` links, counting each tree's root as level 1.
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

pub(crate) fn access_from_tag(tag: u8) -> Result<AccessModifier, DecodeError> {
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

/// A declaration's id -> mixin-table row id, as `encode` assigns them:
/// keyed by id rather than by address, since a class object, an applied
/// clause or a reified mixin may name any version of a declaration, and
/// the row describes the current one.
pub type MixinIds = HashMap<MixinId, u32>;

/// Writes the mixin table of `rows` (`plan`) and returns the id -> row id
/// map the object graph addresses a class object's mixin by. With `texts`,
/// the image carries its source, and each row ends with the index of its
/// declaration's text in the text table (`texts::encode`).
pub fn encode(
    writer: &mut Writer,
    rows: &[Row],
    symbols: &mut symbols::Builder,
    codes: &CodeIds,
    texts: Option<&TextIds>,
) -> Result<MixinIds, String> {
    writer.write_u32(u32_field("mixin table entry count", rows.len()));
    for (index, row) in rows.iter().enumerate() {
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
                writer.write_u16(arity_field("factory arity", factory.argument_count)?);
            }
            None => writer.write_boolean(false),
        }
        writer.write_boolean(mixin.has_initializer_expressions);
        match &mixin.superclass {
            Some(message) => {
                writer.write_boolean(true);
                write_clause_message(writer, *message, symbols);
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
            write_clause_message(writer, clause.message, symbols);
        }
        // Whether the declaration writes a body, or is the mixin
        // application its clauses write (`Mixin::has_body`).
        writer.write_boolean(mixin.has_body);
        writer.write_u32(u32_field("mixin's method count", row.methods.len()));
        for method in &row.methods {
            writer.write_u32(symbols.intern(method.selector.as_str()));
            writer.write_u8(access_tag(method.access));
            writer.write_u16(arity_field("method arity", method.argument_count)?);
            writer.write_u32(codes.id_of(&method.code)?);
        }
        writer.write_u32(u32_field("mixin's nested count", row.nested.len()));
        for id in &row.nested {
            writer.write_u32(*id);
        }
        write_optional_id(writer, row.class_side);
        // The three initializer bodies: the full one, its own part and the
        // standalone one.
        for code in [
            &mixin.initializer_code,
            &mixin.own_initializer_code,
            &mixin.standalone_initializer_code,
        ] {
            write_optional_id(
                writer,
                code.as_ref().map(|code| codes.id_of(code)).transpose()?,
            );
        }
        write_optional_id(
            writer,
            mixin
                .superclass_clause_code
                .as_ref()
                .map(|code| codes.id_of(code))
                .transpose()?,
        );
        if let Some(texts) = texts {
            writer.write_u32(texts.id_of(index, mixin)?);
        }
    }
    Ok(rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| Some((row.id?, u32_field("mixin table id", index))))
        .collect())
}

/// A superclass clause's or a `<:` clause's message: its selector, then
/// whether it carries arguments.
fn write_clause_message(
    writer: &mut Writer,
    message: ClauseMessage,
    symbols: &mut symbols::Builder,
) {
    writer.write_u32(symbols.intern(message.selector.as_str()));
    writer.write_boolean(message.has_arguments);
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
    /// The full instance initializer.
    pub initializer: Option<u32>,
    /// The own part of the instance initializer.
    pub own_initializer: Option<u32>,
    /// The standalone instance initializer.
    pub standalone_initializer: Option<u32>,
    pub superclass_clause: Option<u32>,
    /// The text the row names, when the image carries source.
    pub text: Option<RowText>,
}

/// A row's text as the row names it: its index in the text table, and
/// where in the image the index lies, for `inspect`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowText {
    pub index: u32,
    pub offset: usize,
}

/// The decoded mixin table: the rebuilt declarations, in row-id order (which
/// is `mixin::preorder` order of each module's root, one tree after another),
/// and each one's code bindings at the same index.
#[derive(Debug)]
pub struct Decoded {
    pub mixins: Vec<Rc<Mixin>>,
    pub code: Vec<MixinCode>,
    /// Each tree's first row, in table order: the row of each module's root
    /// declaration, as `check_preorder` found the trees.
    pub module_rows: Vec<u32>,
}

impl Decoded {
    pub fn empty() -> Decoded {
        Decoded {
            mixins: Vec::new(),
            code: Vec::new(),
            module_rows: Vec::new(),
        }
    }

    /// Module `module_index`'s root declaration, the first row of its tree.
    pub fn module_root(&self, module_index: usize) -> Option<&Rc<Mixin>> {
        let row = *self.module_rows.get(module_index)?;
        self.mixins.get(row as usize)
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
    superclass: Option<ClauseMessage>,
    mixin_applications: Vec<ClauseMessage>,
    has_body: bool,
    methods: Vec<(Symbol, AccessModifier, usize, u32)>,
    nested: Vec<u32>,
    class_side: Option<u32>,
    initializer: Option<u32>,
    own_initializer: Option<u32>,
    standalone_initializer: Option<u32>,
    superclass_clause: Option<u32>,
    text: Option<RowText>,
}

/// A slot as read: its name, kind and accessibility.
struct RawSlot {
    name: Symbol,
    mutable: bool,
    lazy: bool,
    access: AccessModifier,
}

/// Reads the mixin table of an image without source and builds its
/// mixins, none of them holding a text: `read`, then `build` with no
/// texts. For the tests, which craft tables of rows that name no text.
pub fn decode(
    reader: &mut Reader,
    symbols: &symbols::Table,
    codes: &[Rc<CompiledCode>],
) -> Result<Decoded, DecodeError> {
    build(read(reader, symbols, codes, false)?, codes, &[])
}

/// The mixin table as `read` leaves it: every row read and checked, the
/// trees found, no mixin built yet.
pub struct ReadTable {
    raw: Vec<RawRow>,
    /// Each tree's first row, in table order: the row of each module's root
    /// declaration, as `check_preorder` found the trees.
    pub module_rows: Vec<u32>,
}

/// Reads the mixin table, and checks it, without building it.
///
/// `codes` is the already-decoded code table: every code id a row names is
/// range-checked against it here, at the point of use, so no later pass can
/// index it unchecked. With `has_source`, each row ends with the index of
/// its text, which `build` checks against the text table.
///
/// Two passes, both iterative, before `build`'s third:
///
/// 1. read every row, validating each count against the remaining input and
///    each symbol and code id against its table;
/// 2. verify that the rows are exactly `mixin::preorder` of each tree's
///    first row, tree after tree -- one explicit-stack walk that
///    simultaneously establishes the rows form a forest (no row is its own
///    ancestor, no row has two parents, no row reaches back into an earlier
///    tree), that no tree is deeper than `MAX_MIXIN_DEPTH`, and where each
///    tree starts.
pub fn read(
    reader: &mut Reader,
    symbols: &symbols::Table,
    codes: &[Rc<CompiledCode>],
    has_source: bool,
) -> Result<ReadTable, DecodeError> {
    let count = reader.read_u32()? as usize;
    let mut raw: Vec<RawRow> = reserve_for(count, reader);
    for _ in 0..count {
        raw.push(read_row(reader, symbols, codes, has_source)?);
    }
    check_ids(&raw)?;
    let module_rows = check_preorder(&raw)?;
    check_member_names(&raw)?;
    check_lazy_slot_getters(&raw)?;
    Ok(ReadTable { raw, module_rows })
}

fn read_row(
    reader: &mut Reader,
    symbols: &symbols::Table,
    codes: &[Rc<CompiledCode>],
    has_source: bool,
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
        let selector = read_symbol(reader, symbols)?;
        let argument_count = reader.read_u16()? as usize;
        check_argument_count("factory", argument_count)?;
        Some(Factory {
            selector,
            argument_count,
        })
    } else {
        None
    };
    let has_initializer_expressions = reader.read_boolean()?;
    let superclass = if reader.read_boolean()? {
        Some(read_clause_message(reader, symbols)?)
    } else {
        None
    };
    let clause_count = reader.read_u32()? as usize;
    if clause_count > crate::mixin::CLAUSE_INDEX_LIMIT {
        return Err(DecodeError::ClauseCountTooLarge {
            count: clause_count,
        });
    }
    let mut mixin_applications: Vec<ClauseMessage> = reserve_for(clause_count, reader);
    for _ in 0..clause_count {
        mixin_applications.push(read_clause_message(reader, symbols)?);
    }
    let has_body = reader.read_boolean()?;
    let method_count = reader.read_u32()? as usize;
    let mut methods: Vec<(Symbol, AccessModifier, usize, u32)> = reserve_for(method_count, reader);
    for _ in 0..method_count {
        let selector = read_symbol(reader, symbols)?;
        let access = access_from_tag(reader.read_u8()?)?;
        let argument_count = reader.read_u16()? as usize;
        check_argument_count("method", argument_count)?;
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
    let own_initializer = read_optional(reader, |reader| read_code_id(reader, codes))?;
    let standalone_initializer = read_optional(reader, |reader| read_code_id(reader, codes))?;
    let superclass_clause = read_optional(reader, |reader| read_code_id(reader, codes))?;
    let text = if has_source {
        let offset = reader.position();
        Some(RowText {
            index: reader.read_u32()?,
            offset,
        })
    } else {
        None
    };
    Ok(RawRow {
        name,
        access,
        span,
        slots,
        factory,
        has_initializer_expressions,
        superclass,
        mixin_applications,
        has_body,
        methods,
        nested,
        class_side,
        initializer,
        own_initializer,
        standalone_initializer,
        superclass_clause,
        text,
    })
}

/// The inverse of `write_clause_message`.
fn read_clause_message(
    reader: &mut Reader,
    symbols: &symbols::Table,
) -> Result<ClauseMessage, DecodeError> {
    Ok(ClauseMessage {
        selector: read_symbol(reader, symbols)?,
        has_arguments: reader.read_boolean()?,
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
    let factory_selectors = metaclass_factory_selectors(raw);
    for (index, row) in raw.iter().enumerate() {
        let factory = factory_selectors[index].into_iter();
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
/// takes an argument or has another accessibility would leave the slot
/// without the getter §6.3.3 gives it. Runs after `check_member_names`, so
/// the method is the only member of that name.
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

/// The rows must be exactly `mixin::preorder` of each tree's root, tree
/// after tree: row `i` is the `i`th declaration those walks reach.
///
/// Row position is how a class object's record names the declaration it
/// applies (`decode::decode_class_record` indexes the rows by id), so this
/// is the property that makes a row's position mean anything at all. It is
/// also, as a by-product, the whole
/// structural sanity check: an index is accepted only when it equals the
/// step counter, so every row is visited exactly once (no shared child, no
/// cycle, no reach back into an earlier tree -- each would revisit an index
/// already behind the counter). Iterative, over an explicit stack, because
/// the depth is the image's to choose.
///
/// A table describing several modules is several such trees, one after
/// another in module order, so the walk starts a new tree wherever the
/// previous one ended; each tree's first row is a module's root, which is
/// how the module section names it. A row nothing reaches therefore starts
/// a tree of its own, and is refused where the trees are counted against
/// the modules the image declares.
fn check_preorder(raw: &[RawRow]) -> Result<Vec<u32>, DecodeError> {
    let mut module_rows = Vec::new();
    let mut expected: u32 = 0;
    while (expected as usize) < raw.len() {
        module_rows.push(expected);
        let mut stack: Vec<(u32, usize)> = vec![(expected, 1)];
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
    }
    Ok(module_rows)
}

/// The factory entry each row holds as a *metaclass* mixin: for a row some
/// other row names as its class side, the entry that other row's own factory
/// defines (`newspeak-spec.pdf` §6.3, "The class side, together with the
/// primary factory method, defines the mixin for the metaclass"); `None` for
/// every other row, which is not a metaclass mixin and must not answer a
/// factory selector.
///
/// The entry is derived rather than written to the image, exactly as a
/// slot's accessors are (§6.3.2): its body is `compiler::factory_body` of
/// the arity and everything else it needs is already in the parent row's
/// factory field, so writing it would be a second copy of the format's own
/// answer, free to disagree with it.
///
/// `check_preorder` has established that every row is reached exactly once,
/// so no row is the class side of two declarations and the answer is unique.
/// Runs after `check_ids`, which is what lets this index `raw`. Built once
/// per decode, by `build`; `check_member_names` asks only for the selectors
/// (`metaclass_factory_selectors`), which build no body.
fn metaclass_factory_entries(raw: &[RawRow]) -> Vec<Option<(Symbol, MethodEntry)>> {
    let mut entries: Vec<Option<(Symbol, MethodEntry)>> = vec![None; raw.len()];
    for row in raw {
        if let Some(class_side) = row.class_side {
            entries[class_side as usize] = Some(crate::mixin::factory_entry(row.factory));
        }
    }
    entries
}

/// The selector of the factory entry `metaclass_factory_entries` gives each
/// row, without the entry: `mixin::factory_entry_selector` of the same
/// factory, so the two cannot disagree.
fn metaclass_factory_selectors(raw: &[RawRow]) -> Vec<Option<Symbol>> {
    let mut selectors: Vec<Option<Symbol>> = vec![None; raw.len()];
    for row in raw {
        if let Some(class_side) = row.class_side {
            selectors[class_side as usize] =
                Some(crate::mixin::factory_entry_selector(row.factory));
        }
    }
    selectors
}

/// Each row's declaration text (`Mixin::text`), the text of the text
/// table its index names; `None` for every row of an image without
/// source.
///
/// A text belongs to one declaration: every text of the table is named by
/// exactly one instance-side row, and a class-side row names exactly its
/// instance side's. A text named by two declarations would be assembled
/// into the module's text once per naming, so a short image could make the
/// load build an arbitrarily large text; a text no row names would be
/// bytes the image carries for nothing. A row is held to the nested rows
/// of the instance side it belongs to: a class side's text is its
/// instance side's, holes and all.
///
/// Holes are held to the nested rows by count, never by span. After an
/// edit of a nested declaration, the enclosing declaration's holes are
/// still in the coordinates of the text it was read in, while the nested
/// row's span and text are in the edited text's; the two are joined by a
/// hole's index (`DeclarationText::assemble`), so a span comparison would
/// refuse every image of an edited program.
///
/// Runs after `check_ids` and `check_preorder`, which are what let this
/// index `raw` by a row's class-side id and know that an instance side's
/// row comes before its class side's.
fn row_texts(
    raw: &[RawRow],
    texts: &[Rc<DeclarationText>],
) -> Result<Vec<Option<Rc<DeclarationText>>>, DecodeError> {
    let row_id = |index: usize| u32::try_from(index).unwrap_or(u32::MAX);
    let mut instance_side: Vec<Option<usize>> = vec![None; raw.len()];
    for (index, row) in raw.iter().enumerate() {
        if let Some(class_side) = row.class_side {
            instance_side[class_side as usize] = Some(index);
        }
    }
    // The instance-side row that names each text, once one has.
    let mut named_by: Vec<Option<usize>> = vec![None; texts.len()];
    let mut row_texts = Vec::with_capacity(raw.len());
    for (index, row) in raw.iter().enumerate() {
        let Some(row_text) = row.text else {
            row_texts.push(None);
            continue;
        };
        let text = texts
            .get(row_text.index as usize)
            .ok_or(DecodeError::TextIndexOutOfRange {
                row: row_id(index),
                index: row_text.index,
                texts: texts.len(),
            })?;
        let owner = match instance_side[index] {
            Some(owner) => {
                let owner_text = raw[owner].text.map(|text| text.index);
                if owner_text != Some(row_text.index) {
                    return Err(DecodeError::ClassSideTextMismatch {
                        row: row_id(index),
                        index: row_text.index,
                        instance_side: row_id(owner),
                    });
                }
                owner
            }
            None => {
                let slot = &mut named_by[row_text.index as usize];
                if let Some(first) = *slot {
                    return Err(DecodeError::TextSharedByRows {
                        text: row_text.index,
                        first: row_id(first),
                        second: row_id(index),
                    });
                }
                *slot = Some(index);
                index
            }
        };
        let nested = raw[owner].nested.len();
        if text.holes().len() != nested {
            return Err(DecodeError::TextHolesMismatch {
                row: row_id(index),
                holes: text.holes().len(),
                nested,
            });
        }
        row_texts.push(Some(Rc::clone(text)));
    }
    if let Some(unnamed) = named_by.iter().position(Option::is_none) {
        return Err(DecodeError::UnnamedText {
            text: row_id(unnamed),
        });
    }
    Ok(row_texts)
}

/// Builds the `Rc<Mixin>`s back to front, the third pass after `read`'s
/// two. `check_preorder` established that every child's row id is greater
/// than its parent's, so a row's children are already built by the time
/// the loop reaches it, and no back-patching pass is needed to close the
/// tree; each child is linked to its parent once the parent's `Rc` exists
/// (`Mixin::enclosing`).
///
/// `texts` is the image's text table, empty for an image without source:
/// each row is given the text its index names, and a row naming a text
/// past the table, another declaration's text or a text with another
/// number of holes than it has nested declarations, and a text no row
/// names, are refused (`row_texts`) before anything is built.
///
/// Each method and each per-mixin body is handed the code-table entry its
/// row names -- `read_row` range-checked every id against `codes` already.
/// Nothing here reconciles a body with what it is attached to:
/// `decode::verify_mixins` does, on the load path only, so `inspect` can
/// still display an artifact whose halves disagree.
pub fn build(
    table: ReadTable,
    codes: &[Rc<CompiledCode>],
    texts: &[Rc<DeclarationText>],
) -> Result<Decoded, DecodeError> {
    let ReadTable { raw, module_rows } = table;
    let code_at = |id: u32| codes[id as usize].clone();
    let mut texts = row_texts(&raw, texts)?;
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
                        // A lazy slot's getter needs none either: its body
                        // answers the filled slot (`ReturnSlotIfFilled`).
                        fast_path: std::cell::Cell::new(None),
                        recognized_fast_path: None,
                        // A class accessor is recognized in shipped source alone.
                        class_accessor: None,
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
            own_initializer: row.own_initializer,
            standalone_initializer: row.standalone_initializer,
            superclass_clause: row.superclass_clause,
            text: row.text,
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
            superclass_receiver: None,
            mixin_applications: row
                .mixin_applications
                .iter()
                .map(|&message| MixinApplicationClause {
                    message,
                    receiver: None,
                })
                .collect(),
            has_body: row.has_body,
            has_initializer_expressions: row.has_initializer_expressions,
            initializer_expressions: Vec::new(),
            nested_declarations: crate::mixin::NestedDeclarations::new(nested),
            class_side,
            span: row.span,
            declaration: None,
            initializer_code: row.initializer.map(code_at),
            own_initializer_code: row.own_initializer.map(code_at),
            standalone_initializer_code: row.standalone_initializer.map(code_at),
            superclass_clause_code: row.superclass_clause.map(code_at),
            // The loader refuses a root class record the root rule forbids
            // (`modules::root_record_defect`), so no decoded mixin has one.
            root_clause_defect: None,
            root_shape: None,
            // An image carries only its own module, never shipped source.
            shipped: false,
            // Set by the parent's row, built after this one.
            enclosing: std::cell::OnceCell::new(),
            text: texts[index].take(),
        });
        Mixin::set_enclosing_of_children(&mixin);
        mixin.mark_declaration_code();
        built[index] = Some(mixin);
    }
    Ok(Decoded {
        mixins: built
            .into_iter()
            .map(|mixin| mixin.expect("every row was built"))
            .collect(),
        code: code
            .into_iter()
            .map(|code| code.expect("every row was built"))
            .collect(),
        module_rows,
    })
}
