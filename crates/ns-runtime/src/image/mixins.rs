//! The image's mixin table: one row per class declaration, flat.
//!
//! A `Mixin` is what a loaded image has to reconstruct before anything can
//! run -- the method dictionary a send is looked up in, the slot names an
//! instance is laid out by, the factory selector §6.1 checks a superclass
//! clause against. Before this section existed those came back only by
//! re-parsing the image's own source text and rebuilding the tree from the
//! AST, which made the front end part of the load path and made the image's
//! own bytes an incomplete description of the world they claim to hold.
//! This table is what retired that re-parse: `image::load` builds its whole
//! declaration tree, and its root class, from these rows.
//!
//! **The tree is flattened, not nested.** Rows are written in
//! `mixin::preorder` order -- the same order `compiler::compile_module`
//! walks the tree in, which is what lets `decode::build_module` pair a row
//! with its own compiled bodies -- and a row names its nested declarations
//! and its class side by row id. Nothing in the byte
//! stream is nested inside anything else, so decoding is a loop over rows
//! and never a recursion on a depth the image controls. The one structural
//! recursion left in the runtime (`mixin::preorder`) is protected by
//! `MAX_MIXIN_DEPTH` below, which is also what keeps dropping a decoded tree
//! -- itself a recursive walk of `Rc<Mixin>` children -- inside the stack.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast::AccessModifier;
use ns_syntax::Span;

use crate::bytecode::{method_key, CompiledCode, CompiledModule};
use crate::mixin::{mixin_key, Factory, Mixin, MixinMethod, MixinSlot, SuperclassClause};
use crate::types::{ClassRef, ObjectData};
use crate::Symbol;

use super::decode::{reserve_for, DecodeError, Reader};
use super::encode::{u32_field, CodeIds, Writer};
use super::symbols;

/// `ns-syntax`'s own nesting ceiling: the parser refuses a module whose
/// classes nest deeper than this (`MAX_DEPTH` in
/// `crates/ns-syntax/src/parser/mod.rs`, which is private to that crate, so
/// the number has to be restated rather than imported).
///
/// It counts *nested* classes only: `parser/decl.rs` calls `Parser::enter`
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

/// One declaration's row, with its children already resolved to row ids and
/// its code already resolved to the `CompiledCode` the code table will
/// assign an id to.
pub struct Row {
    pub mixin: Rc<Mixin>,
    pub nested: Vec<u32>,
    pub class_side: Option<u32>,
    /// Sorted by selector text: the mixin's method table is a `HashMap`, and
    /// sorting is what makes the row canonical rather than dependent on that
    /// map's iteration order.
    pub methods: Vec<(Rc<MixinMethod>, Rc<CompiledCode>)>,
    pub initializer: Option<Rc<CompiledCode>>,
    pub superclass_clause: Option<Rc<CompiledCode>>,
    pub superclass_message: Option<Rc<CompiledCode>>,
}

/// Flattens the tree under `root_class` into rows, in `mixin::preorder`
/// order, resolving each declaration's compiled code out of `module`.
///
/// A method a mixin holds but the `CompiledModule` has no code for is an
/// `Err`, not a panic: a method added to a live class after `compile_module`
/// ran is exactly what the liveness spec's Req. 1 anticipates, so it is a
/// refusal to snapshot this world.
///
/// Called by both `encode::encode_code_table` (which needs the rows to know
/// which `CompiledCode` is reachable, and in what order) and `encode` below
/// (which needs them to write the table). Recomputing it is one tree walk
/// and one `HashMap`; sharing a cached copy between the two would trade that
/// for a lifetime that has to thread through `snapshot`.
pub fn plan(root_class: &ClassRef, module: &CompiledModule) -> Result<Vec<Row>, String> {
    let root_mixin = {
        let borrowed = root_class.borrow();
        match &borrowed.data {
            ObjectData::Class(data) => Some(data.application.mixin.clone()),
            _ => None,
        }
    };
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
        .map(|(i, mixin)| (mixin_key(mixin), u32_field("mixin table id", i)))
        .collect();
    let mut rows = Vec::with_capacity(order.len());
    for mixin in &order {
        let mut methods: Vec<(Rc<MixinMethod>, Rc<CompiledCode>)> = mixin
            .methods
            .values()
            .map(|method| {
                let code = module.methods.get(&method_key(method)).ok_or_else(|| {
                    format!(
                        "image encoding has no compiled code for method #{} \
                         (it was added to the class after the module was compiled)",
                        method.selector
                    )
                })?;
                Ok((method.clone(), code.clone()))
            })
            .collect::<Result<Vec<_>, String>>()?;
        methods.sort_by(|a, b| a.0.selector.as_str().cmp(b.0.selector.as_str()));
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
            initializer: module.initializers.get(&mixin_key(mixin)).cloned(),
            superclass_clause: module.superclass_clauses.get(&mixin_key(mixin)).cloned(),
            superclass_message: module.superclass_messages.get(&mixin_key(mixin)).cloned(),
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
    for (i, row) in rows.iter().enumerate().rev() {
        let deepest_child = row
            .nested
            .iter()
            .chain(row.class_side.iter())
            .map(|id| depth[*id as usize])
            .max()
            .unwrap_or(0);
        depth[i] = deepest_child + 1;
    }
    depth.into_iter().max().unwrap_or(0)
}

/// Explicit wire tags for `AccessModifier` -- never `as u8` on the enum,
/// whose declaration order is not a wire format.
fn access_tag(access: AccessModifier) -> u8 {
    match access {
        AccessModifier::Public => 0,
        AccessModifier::Protected => 1,
        AccessModifier::Private => 2,
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

/// Writes the mixin table and returns the `mixin_key` -> row id map the
/// object graph addresses a class object's mixin by (Task 13).
pub fn encode(
    w: &mut Writer,
    root_class: &ClassRef,
    module: &CompiledModule,
    symbols: &mut symbols::Builder,
    codes: &CodeIds,
) -> Result<HashMap<usize, u32>, String> {
    let rows = plan(root_class, module)?;
    w.write_u32(u32_field("mixin table entry count", rows.len()));
    for row in &rows {
        let mixin = &row.mixin;
        w.write_u32(symbols.intern(mixin.name.as_str()));
        w.write_u8(access_tag(mixin.access));
        w.write_u32(mixin.span.start);
        w.write_u32(mixin.span.end);
        w.write_u32(u32_field("mixin's slot count", mixin.slots.len()));
        for slot in &mixin.slots {
            w.write_u32(symbols.intern(slot.name.as_str()));
        }
        match &mixin.factory {
            Some(factory) => {
                w.write_bool(true);
                w.write_u32(symbols.intern(factory.selector.as_str()));
                w.write_u16(u16_arity("factory", factory.num_args));
            }
            None => w.write_bool(false),
        }
        w.write_bool(mixin.has_init_exprs);
        match &mixin.superclass {
            Some(clause) => {
                w.write_bool(true);
                w.write_u32(symbols.intern(clause.selector.as_str()));
                w.write_bool(clause.has_args);
            }
            None => w.write_bool(false),
        }
        w.write_u32(u32_field("mixin's method count", row.methods.len()));
        for (method, code) in &row.methods {
            w.write_u32(symbols.intern(method.selector.as_str()));
            w.write_u8(access_tag(method.access));
            w.write_u16(u16_arity("method", method.num_args));
            w.write_u32(codes.id_of(code));
        }
        w.write_u32(u32_field("mixin's nested count", row.nested.len()));
        for id in &row.nested {
            w.write_u32(*id);
        }
        write_optional_id(w, row.class_side);
        write_optional_id(w, row.initializer.as_ref().map(|c| codes.id_of(c)));
        write_optional_id(w, row.superclass_clause.as_ref().map(|c| codes.id_of(c)));
        write_optional_id(w, row.superclass_message.as_ref().map(|c| codes.id_of(c)));
    }
    Ok(rows
        .iter()
        .enumerate()
        .map(|(i, row)| (mixin_key(&row.mixin), u32_field("mixin table id", i)))
        .collect())
}

/// A parameter count narrowed to the `u16` the wire (and `CompiledCode`)
/// stores an arity in. A pattern with more than 65535 keyword parts is not
/// something any parser produces; writing a truncated one would make the
/// image self-inconsistent, so it aborts rather than truncating silently --
/// the same contract as `encode::u32_field`.
fn u16_arity(what: &str, n: usize) -> u16 {
    assert!(
        u16::try_from(n).is_ok(),
        "image {what} arity of {n} does not fit the format's u16 field"
    );
    n as u16
}

fn write_optional_id(w: &mut Writer, id: Option<u32>) {
    match id {
        Some(id) => {
            w.write_bool(true);
            w.write_u32(id);
        }
        None => w.write_bool(false),
    }
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// One decoded row's *code* half: which compiled body belongs to which of
/// the mixin's members. Kept beside the rebuilt `Mixin` rather than on it,
/// because a `Mixin` is a description of a declaration and a code-table id
/// is a fact about one image.
#[derive(Debug, PartialEq)]
pub struct MixinCode {
    /// `(selector, code id)`, sorted by selector exactly as encoded.
    pub methods: Vec<(Symbol, u32)>,
    pub initializer: Option<u32>,
    pub superclass_clause: Option<u32>,
    pub superclass_message: Option<u32>,
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
    slots: Vec<Symbol>,
    factory: Option<Factory>,
    has_init_exprs: bool,
    superclass: Option<SuperclassClause>,
    methods: Vec<(Symbol, AccessModifier, usize, u32)>,
    nested: Vec<u32>,
    class_side: Option<u32>,
    initializer: Option<u32>,
    superclass_clause: Option<u32>,
    superclass_message: Option<u32>,
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
    r: &mut Reader,
    symbols: &symbols::Table,
    codes: &[Rc<CompiledCode>],
) -> Result<Decoded, DecodeError> {
    let count = r.read_u32()? as usize;
    let mut raw: Vec<RawRow> = reserve_for(count, r);
    for _ in 0..count {
        raw.push(read_row(r, symbols, codes)?);
    }
    if raw.is_empty() {
        return Ok(Decoded::empty());
    }
    check_ids(&raw)?;
    check_preorder(&raw)?;
    Ok(build(raw))
}

fn read_row(
    r: &mut Reader,
    symbols: &symbols::Table,
    codes: &[Rc<CompiledCode>],
) -> Result<RawRow, DecodeError> {
    let name = read_symbol(r, symbols)?;
    let access = access_from_tag(r.read_u8()?)?;
    let span = read_span(r)?;
    let slot_count = r.read_u32()? as usize;
    let mut slots: Vec<Symbol> = reserve_for(slot_count, r);
    for _ in 0..slot_count {
        slots.push(read_symbol(r, symbols)?);
    }
    let factory = if r.read_bool()? {
        Some(Factory {
            selector: read_symbol(r, symbols)?,
            num_args: r.read_u16()? as usize,
        })
    } else {
        None
    };
    let has_init_exprs = r.read_bool()?;
    let superclass = if r.read_bool()? {
        Some(SuperclassClause {
            selector: read_symbol(r, symbols)?,
            has_args: r.read_bool()?,
        })
    } else {
        None
    };
    let method_count = r.read_u32()? as usize;
    let mut methods: Vec<(Symbol, AccessModifier, usize, u32)> = reserve_for(method_count, r);
    for _ in 0..method_count {
        let selector = read_symbol(r, symbols)?;
        let access = access_from_tag(r.read_u8()?)?;
        let num_args = r.read_u16()? as usize;
        let code = read_code_id(r, codes)?;
        // A row that names one selector twice would build a method
        // dictionary with fewer entries than its code list has, and
        // `decode::build_module` pairs the two up entry by entry. The
        // encoder writes each selector once, sorted, so this is only
        // reachable from a crafted image.
        if methods.iter().any(|(existing, ..)| *existing == selector) {
            return Err(DecodeError::DuplicateSelector(
                selector.as_str().to_string(),
            ));
        }
        methods.push((selector, access, num_args, code));
    }
    let nested_count = r.read_u32()? as usize;
    let mut nested: Vec<u32> = reserve_for(nested_count, r);
    for _ in 0..nested_count {
        nested.push(r.read_u32()?);
    }
    let class_side = read_optional(r, |r| r.read_u32())?;
    let initializer = read_optional(r, |r| read_code_id(r, codes))?;
    let superclass_clause = read_optional(r, |r| read_code_id(r, codes))?;
    let superclass_message = read_optional(r, |r| read_code_id(r, codes))?;
    Ok(RawRow {
        name,
        access,
        span,
        slots,
        factory,
        has_init_exprs,
        superclass,
        methods,
        nested,
        class_side,
        initializer,
        superclass_clause,
        superclass_message,
    })
}

fn read_symbol(r: &mut Reader, symbols: &symbols::Table) -> Result<Symbol, DecodeError> {
    symbols.get(r.read_u32()?)
}

fn read_span(r: &mut Reader) -> Result<Span, DecodeError> {
    let start = r.read_u32()?;
    let end = r.read_u32()?;
    if start > end {
        return Err(DecodeError::InvalidSpan { start, end });
    }
    Ok(Span::new(start, end))
}

fn read_code_id(r: &mut Reader, codes: &[Rc<CompiledCode>]) -> Result<u32, DecodeError> {
    let id = r.read_u32()?;
    if (id as usize) < codes.len() {
        Ok(id)
    } else {
        Err(DecodeError::BadCodeId(id))
    }
}

fn read_optional<T>(
    r: &mut Reader,
    read: impl FnOnce(&mut Reader) -> Result<T, DecodeError>,
) -> Result<Option<T>, DecodeError> {
    if r.read_bool()? {
        Ok(Some(read(r)?))
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

/// The rows must be exactly `mixin::preorder(row 0)`: row `i` is the `i`th
/// declaration that walk reaches.
///
/// Row position is how a class object.s record names the declaration it
/// applies (`decode::decode_class_record` indexes the rows by id), so this
/// is the property that makes a row.s position mean anything at all. It is also, as a by-product, the whole
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

/// Builds the `Rc<Mixin>`s back to front. `check_preorder` established that
/// every child's row id is greater than its parent's, so a row's children
/// are already built by the time the loop reaches it.
fn build(raw: Vec<RawRow>) -> Decoded {
    let mut built: Vec<Option<Rc<Mixin>>> = vec![None; raw.len()];
    let mut code: Vec<Option<MixinCode>> = (0..raw.len()).map(|_| None).collect();
    for (i, row) in raw.into_iter().enumerate().rev() {
        let methods = row
            .methods
            .iter()
            .map(|&(selector, access, num_args, _)| {
                (
                    selector,
                    Rc::new(MixinMethod {
                        selector,
                        access,
                        num_args,
                        // No declaration to point at, and none invented:
                        // the compiled body in the code table is what this
                        // method runs, and the tree-walking engine -- the
                        // only reader of a declaration at run time -- cannot
                        // run from an image at all.
                        decl: None,
                    }),
                )
            })
            .collect();
        code[i] = Some(MixinCode {
            methods: row
                .methods
                .iter()
                .map(|&(selector, _, _, id)| (selector, id))
                .collect(),
            initializer: row.initializer,
            superclass_clause: row.superclass_clause,
            superclass_message: row.superclass_message,
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
            slots: row
                .slots
                .into_iter()
                .map(|name| MixinSlot { name, decl: None })
                .collect(),
            factory: row.factory,
            superclass: row.superclass,
            has_init_exprs: row.has_init_exprs,
            init_exprs: Vec::new(),
            nested,
            class_side,
            span: row.span,
            decl: None,
        });
        built[i] = Some(mixin);
    }
    Decoded {
        mixins: built
            .into_iter()
            .map(|m| m.expect("every row was built"))
            .collect(),
        code: code
            .into_iter()
            .map(|c| c.expect("every row was built"))
            .collect(),
    }
}
