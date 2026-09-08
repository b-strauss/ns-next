//! Canonical, little-endian, self-describing byte encoding for the image
//! format -- the spec's "Image format" section. Never a memory dump.

use std::collections::HashMap;
use std::rc::Rc;

use crate::bytecode::{CallSite, CodeKind, CompiledCode, CompiledModule, Instr, Literal};
use crate::heap::Heap;
use crate::types::{
    BuiltinClass, ClassData, ClassRef, ClosureBody, ClosureData, FrameKind, FrameRef, ObjRef,
    ObjectData, PcKind, SourceOrigin, Value,
};

use super::opcodes::*;
use super::{mixins, symbols};

pub const MAGIC: &[u8; 4] = b"NSIM";
/// Versions the whole layout *and* the compiler's codegen: from the first
/// image any released build can write, any change to `Instr`, to what the
/// compiler emits, or to any section of the layout bumps it. A mismatch is
/// refused on load, never migrated (Milestone 2 Non-Goals).
///
/// Version 1 covers the layout as `image::snapshot` finally writes it,
/// source and `SourceOrigin` sections included. Those two arrived after the
/// constant did, but within this milestone: until `snapshot` existed no
/// build wrote a whole image, so no reader can hold a version-1 image
/// without them and there is nothing for a bump to distinguish.
///
/// The header's flag byte (`FLAG_HAS_SOURCE`) is what carries variation
/// *within* a version from here on: the source blob is optional, and a
/// reader distinguishes the two variants by the flag rather than by the
/// version. A flag bit this build does not know is refused
/// (`DecodeError::UnknownHeaderFlags`), which is what keeps a future
/// optional section from being silently skipped by an older reader.
///
/// Literal tag 9 (`UnresolvedIdent`) was added by the same reasoning, and
/// deliberately: it is a pure addition to the literal tag space made
/// before any released build could write an image, so no version-1 image
/// in anyone's hands can be misread because of it. The next such change
/// after a release bumps the version.
///
/// Milestone 3's layout changes -- the symbol table, the mixin table that
/// took the per-mixin sections out of the code table, and the class-object
/// record that retired the `UserRoot` class tag -- do not bump it either,
/// by an explicit decision (2026-09-06): the project has no external
/// users, so images written before them are disposable and no migration path
/// is owed to them. What still has to hold is that an old or corrupt image
/// fails as a `DecodeError`, which comes from validating every id, tag,
/// count and length before it indexes, allocates or slices -- not from this
/// field.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer { buf: Vec::new() }
    }

    pub fn write_raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn write_bool(&mut self, v: bool) {
        self.write_u8(u8::from(v));
    }

    pub fn write_u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// `u32` byte-count prefix, then the raw bytes. A payload too long for
    /// the prefix would write a length that does not describe what follows
    /// -- a self-inconsistent image no reader could detect -- so it aborts
    /// rather than truncating silently.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.write_u32(u32_field("byte string length", bytes.len()));
        self.write_raw(bytes);
    }

    pub fn write_str(&mut self, s: &str) {
        self.write_bytes(s.as_bytes());
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

/// Header flag bits. Bit 0 says a source blob follows the header; every
/// bit this build has not assigned a meaning to is reserved and must be
/// zero, so a reader of this build refuses an image using a flag it does
/// not implement rather than skipping a section it cannot see.
pub const FLAG_HAS_SOURCE: u8 = 1;

/// The artifact carries no object graph: it is a deployment binary, not a
/// snapshot of a running world. Its entry field is mandatory rather than
/// optional, which is what makes it startable from the file alone.
pub const FLAG_STATELESS: u8 = 2;

/// Every bit this build assigns a meaning to.
pub const KNOWN_FLAGS: u8 = FLAG_HAS_SOURCE | FLAG_STATELESS;

pub fn write_header(w: &mut Writer, flags: u8) {
    w.write_raw(MAGIC);
    w.write_u32(FORMAT_VERSION);
    w.write_u8(flags);
}

/// The module's provenance. Kept in every image, source blob or not: it is
/// what a loaded world reports as its own origin, and it is a path, not the
/// program text.
pub fn encode_source_origin(w: &mut Writer, origin: &SourceOrigin) {
    match origin {
        SourceOrigin::File(path) => {
            w.write_u8(0);
            w.write_str(&path.to_string_lossy());
        }
    }
}

/// Encodes the four non-`Obj` `Value` variants (tags 0-4); returns `false`
/// for `Value::Obj`, which `encode_value` (the object-graph encoder) handles.
pub fn encode_scalar_value(w: &mut Writer, v: &Value) -> bool {
    match v {
        Value::Nil => w.write_u8(VAL_NIL),
        Value::Bool(false) => w.write_u8(VAL_FALSE),
        Value::Bool(true) => w.write_u8(VAL_TRUE),
        Value::SmallInt(n) => {
            w.write_u8(VAL_SMALL_INT);
            w.write_i64(*n);
        }
        Value::Float(f) => {
            w.write_u8(VAL_FLOAT);
            w.write_f64(*f);
        }
        Value::Obj(_) => return false,
    }
    true
}

/// A `usize` count or index narrowed to the wire's `u32` field, refusing to
/// truncate: a length prefix that does not describe the payload that
/// follows would make the image self-inconsistent, which no reader could
/// detect. Unreachable for any image a real program can produce -- an
/// invariant an image write must not violate, so it aborts loudly.
pub(crate) fn u32_field(what: &str, n: usize) -> u32 {
    assert!(
        u32::try_from(n).is_ok(),
        "image {what} of {n} does not fit the format's u32 field"
    );
    n as u32
}

/// `u32_field`'s `u16` counterpart, for the register and parameter counts.
fn u16_field(what: &str, n: usize) -> u16 {
    assert!(
        u16::try_from(n).is_ok(),
        "image {what} of {n} does not fit the format's u16 field"
    );
    n as u16
}

/// `Rc<CompiledCode>` pointer -> code-table id, assigned by
/// `encode_code_table`; `encode_closure_record` uses it for a
/// closure's code reference.
pub struct CodeIds {
    ids: HashMap<usize, u32>,
}

impl CodeIds {
    pub fn id_of(&self, code: &Rc<CompiledCode>) -> u32 {
        *self.ids.get(&(Rc::as_ptr(code) as usize)).expect(
            "every CompiledCode reachable from the module was assigned an id by encode_code_table",
        )
    }
}

/// Explicit wire tags for `BuiltinClass` -- never `as u8` on the enum,
/// whose declaration order is not a wire format.
pub fn builtin_class_tag(kind: BuiltinClass) -> u8 {
    match kind {
        BuiltinClass::Object => 0,
        BuiltinClass::Boolean => 1,
        BuiltinClass::True => 2,
        BuiltinClass::False => 3,
        BuiltinClass::Integer => 4,
        BuiltinClass::Float => 5,
        BuiltinClass::Str => 6,
        BuiltinClass::Sym => 7,
        BuiltinClass::Character => 8,
        BuiltinClass::Array => 9,
        BuiltinClass::OrderedCollection => 10,
        BuiltinClass::Nil => 11,
        BuiltinClass::Block => 12,
        BuiltinClass::Platform => 13,
    }
}

/// The module's root class object, which `load` rebuilds *before* the
/// object graph (the graph's class tag 1 names it, so it has to exist by
/// then). Everything else about it comes from mixin-table row 0: this
/// section carries the one thing a declaration does not determine, the
/// class it inherits from.
///
/// Written as an optional builtin-class tag rather than as a general class
/// tag, because the general form names an object-table id and the object
/// table is not decoded yet. That costs nothing today:
/// `instantiate::resolve_superclass_receiver` -- the only thing that ever
/// fixes a root class's superclass -- resolves a builtin class name or
/// nothing at all, so `Some(builtin)` and `None` are the complete set of
/// worlds this runtime builds. A root class inheriting from a *user* class
/// is refused here rather than written wrongly; extending
/// `resolve_superclass_receiver` is what would make it representable, and
/// that extension is where this format would grow with it.
pub fn encode_root_class(w: &mut Writer, heap: &Heap, root_class: &ClassRef) -> Result<(), String> {
    if !heap.is_class(root_class) {
        return Err("image encoding's root object is not a class".to_string());
    }
    let superclass = heap.class_superclass(root_class);
    match superclass {
        None => w.write_bool(false),
        Some(superclass) => {
            let builtin = heap.class_builtin(&superclass);
            let Some(kind) = builtin else {
                return Err(
                    "image encoding cannot serialize a root class whose superclass is a \
                     user-defined class: the format names the root's superclass by builtin \
                     tag, decoded before the object table exists"
                        .to_string(),
                );
            };
            w.write_bool(true);
            w.write_u8(builtin_class_tag(kind));
        }
    }
    Ok(())
}

/// The entry selector, as a presence byte and -- when present -- a symbol id.
///
/// One shape for both artifacts on purpose: the decoder reads it the same way
/// either way and only the *requirement* differs, so there is no second
/// code path to keep in step. It sits after the root class rather than in the
/// header because it names a symbol and must therefore follow the symbol
/// table, and because down here it leaves the describing sections byte-equal
/// between a `build` and a `snapshot` of the same source.
pub fn encode_entry(w: &mut Writer, entry: Option<u32>) {
    match entry {
        None => w.write_u8(0),
        Some(id) => {
            w.write_u8(1);
            w.write_u32(id);
        }
    }
}

/// The code table: every `CompiledCode` reachable from `module` -- every
/// mixin's methods, every mixin's initializer, every superclass clause and
/// clause-message body, and every nested block body -- with nested block
/// code *before* the code that creates it. Flat and unnamed: *which* body
/// belongs to which declaration is the mixin table's business
/// (`image::mixins`), which addresses this table by id.
///
/// Collected in `mixin::preorder` order -- the same order `compile_module`
/// compiled in and the same order the mixin table's rows are written in --
/// so the same module always encodes to the same bytes.
///
/// A method a mixin holds but the `CompiledModule` has no code for is an
/// `Err`, not a panic: a method added to a live class after `compile_module`
/// ran is exactly what the liveness spec's Req. 1 anticipates, so it is a
/// refusal to snapshot this world, on the same footing as
/// `encode_closure_record`'s tree-walker closure.
pub fn encode_code_table(
    w: &mut Writer,
    heap: &Heap,
    module: &CompiledModule,
    root_class: &ClassRef,
    symbols: &mut symbols::Builder,
) -> Result<CodeIds, String> {
    let mut order: Vec<Rc<CompiledCode>> = Vec::new();
    let mut ids = CodeIds {
        ids: HashMap::new(),
    };
    let rows = mixins::plan(heap, root_class, module)?;
    for row in &rows {
        for (_, code) in &row.methods {
            collect_code(code, &mut order, &mut ids);
        }
        for code in [
            &row.initializer,
            &row.superclass_clause,
            &row.superclass_message,
        ]
        .into_iter()
        .flatten()
        {
            collect_code(code, &mut order, &mut ids);
        }
    }
    w.write_u32(u32_field("code table entry count", order.len()));
    for code in &order {
        encode_code(w, code, &ids, symbols);
    }
    Ok(ids)
}

/// Post-order: a block's code is appended (and gets its id) before the
/// code containing its `MakeClosure`, so every `code` operand encodes as an
/// id that is already assigned -- at any nesting depth, since the recursion
/// bottoms out at a body with no `MakeClosure` of its own.
fn collect_code(code: &Rc<CompiledCode>, order: &mut Vec<Rc<CompiledCode>>, ids: &mut CodeIds) {
    let key = Rc::as_ptr(code) as usize;
    if ids.ids.contains_key(&key) {
        return;
    }
    for instr in &code.instrs {
        if let Instr::MakeClosure { code: block, .. } = instr {
            collect_code(block, order, ids);
        }
    }
    ids.ids.insert(key, u32_field("code table id", order.len()));
    order.push(code.clone());
}

fn encode_code(w: &mut Writer, code: &CompiledCode, ids: &CodeIds, symbols: &mut symbols::Builder) {
    w.write_u8(match code.kind {
        CodeKind::Method => 0,
        CodeKind::Block => 1,
        CodeKind::Initializer => 2,
        CodeKind::SuperclassClause => 3,
        CodeKind::SuperclassMessage => 4,
    });
    w.write_u16(code.num_args);
    w.write_u16(code.num_registers);
    w.write_u32(code.source.start);
    w.write_u32(code.source.end);
    w.write_u32(u32_field("statement map length", code.statement_pc.len()));
    for (stmt, range) in &code.statement_pc {
        w.write_u32(u32_field("statement index", *stmt));
        w.write_u32(u32_field("statement pc", range.start));
        w.write_u32(u32_field("statement pc", range.end));
    }
    w.write_u32(u32_field("call site count", code.call_sites.len()));
    for site in &code.call_sites {
        encode_call_site(w, site);
    }
    w.write_u32(u32_field("instruction count", code.instrs.len()));
    for instr in &code.instrs {
        encode_instr(w, instr, ids, symbols);
    }
}

fn encode_call_site(w: &mut Writer, site: &CallSite) {
    w.write_u16(site.call_site_id);
}

fn encode_regs(w: &mut Writer, regs: &[u16]) {
    w.write_u16(u16_field("register list length", regs.len()));
    for r in regs {
        w.write_u16(*r);
    }
}

fn encode_instr(w: &mut Writer, instr: &Instr, ids: &CodeIds, symbols: &mut symbols::Builder) {
    match instr {
        Instr::LoadLiteral { dst, literal } => {
            w.write_u8(OP_LOAD_LITERAL);
            w.write_u16(*dst);
            encode_literal(w, literal, symbols);
        }
        Instr::LoadSelf { dst } => {
            w.write_u8(OP_LOAD_SELF);
            w.write_u16(*dst);
        }
        Instr::LoadBuiltinClass { dst, kind } => {
            w.write_u8(OP_LOAD_BUILTIN_CLASS);
            w.write_u16(*dst);
            w.write_u8(builtin_class_tag(*kind));
        }
        Instr::Move { dst, src } => {
            w.write_u8(OP_MOVE);
            w.write_u16(*dst);
            w.write_u16(*src);
        }
        Instr::StoreLocal { src, slot } => {
            w.write_u8(OP_STORE_LOCAL);
            w.write_u16(*src);
            w.write_u16(*slot);
        }
        Instr::LoadOuterLocal { dst, depth, index } => {
            w.write_u8(OP_LOAD_OUTER_LOCAL);
            w.write_u16(*dst);
            w.write_u16(*depth);
            w.write_u16(*index);
        }
        Instr::StoreOuterLocal { src, depth, index } => {
            w.write_u8(OP_STORE_OUTER_LOCAL);
            w.write_u16(*src);
            w.write_u16(*depth);
            w.write_u16(*index);
        }
        Instr::SendLexical {
            depth,
            selector,
            arg_regs,
            dst,
            call_site,
        } => {
            w.write_u8(OP_SEND_LEXICAL);
            w.write_u16(*depth);
            w.write_u32(symbols.intern(selector.as_str()));
            encode_regs(w, arg_regs);
            w.write_u16(*dst);
            encode_call_site(w, call_site);
        }
        Instr::SendPublic {
            selector,
            receiver_reg,
            arg_regs,
            dst,
            call_site,
        } => {
            w.write_u8(OP_SEND_PUBLIC);
            w.write_u32(symbols.intern(selector.as_str()));
            w.write_u16(*receiver_reg);
            encode_regs(w, arg_regs);
            w.write_u16(*dst);
            encode_call_site(w, call_site);
        }
        Instr::SendSuper {
            selector,
            arg_regs,
            dst,
            call_site,
        } => {
            w.write_u8(OP_SEND_SUPER);
            w.write_u32(symbols.intern(selector.as_str()));
            encode_regs(w, arg_regs);
            w.write_u16(*dst);
            encode_call_site(w, call_site);
        }
        Instr::MakeClosure { dst, params, code } => {
            w.write_u8(OP_MAKE_CLOSURE);
            w.write_u16(*dst);
            w.write_u16(u16_field("closure parameter count", params.len()));
            for p in params.iter() {
                w.write_u32(symbols.intern(p.as_str()));
            }
            w.write_u32(ids.id_of(code));
        }
        Instr::MakeArray { dst, elem_regs } => {
            w.write_u8(OP_MAKE_ARRAY);
            w.write_u16(*dst);
            encode_regs(w, elem_regs);
        }
        Instr::InitSlot { src, own_index } => {
            w.write_u8(OP_INIT_SLOT);
            w.write_u16(*src);
            w.write_u32(*own_index);
        }
        Instr::Return { reg } => {
            w.write_u8(OP_RETURN);
            w.write_u16(*reg);
        }
        Instr::ImplicitReturn { reg } => {
            w.write_u8(OP_IMPLICIT_RETURN);
            w.write_u16(*reg);
        }
        Instr::NonLocalReturn { reg } => {
            w.write_u8(OP_NON_LOCAL_RETURN);
            w.write_u16(*reg);
        }
        Instr::Jump { target } => {
            w.write_u8(OP_JUMP);
            w.write_u32(*target);
        }
        Instr::JumpIfNotBoolean { reg, target } => {
            w.write_u8(OP_JUMP_IF_NOT_BOOLEAN);
            w.write_u16(*reg);
            w.write_u32(*target);
        }
        Instr::BranchIfFalse { reg, target } => {
            w.write_u8(OP_BRANCH_IF_FALSE);
            w.write_u16(*reg);
            w.write_u32(*target);
        }
        Instr::BranchIfTrue { reg, target } => {
            w.write_u8(OP_BRANCH_IF_TRUE);
            w.write_u16(*reg);
            w.write_u32(*target);
        }
    }
}

fn encode_literal(w: &mut Writer, literal: &Literal, symbols: &mut symbols::Builder) {
    match literal {
        Literal::Nil => w.write_u8(LIT_NIL),
        Literal::Bool(b) => {
            w.write_u8(LIT_BOOL);
            w.write_bool(*b);
        }
        Literal::SmallInt(n) => {
            w.write_u8(LIT_SMALL_INT);
            w.write_i64(*n);
        }
        Literal::Float(f) => {
            w.write_u8(LIT_FLOAT);
            w.write_f64(*f);
        }
        Literal::Str(s) => {
            w.write_u8(LIT_STR);
            w.write_str(s);
        }
        Literal::Sym(s) => {
            w.write_u8(LIT_SYM);
            w.write_u32(symbols.intern(s.as_str()));
        }
        Literal::Char(c) => {
            w.write_u8(LIT_CHAR);
            w.write_u32(*c as u32);
        }
        Literal::Array(elements) => {
            w.write_u8(LIT_ARRAY);
            w.write_u32(u32_field("literal array length", elements.len()));
            for e in elements {
                encode_literal(w, e, symbols);
            }
        }
        Literal::MalformedInt(text) => {
            w.write_u8(LIT_MALFORMED_INT);
            w.write_str(text);
        }
        Literal::UnresolvedIdent(name) => {
            w.write_u8(LIT_UNRESOLVED_IDENT);
            w.write_str(name);
        }
    }
}

// ---------------------------------------------------------------------------
// The object table and the frame table (the spec's "Object table", "Frame
// table" and "Root/special-objects table" bullets).
// ---------------------------------------------------------------------------

fn is_same_object(a: &Value, b: &Value) -> bool {
    matches!((a, b), (Value::Obj(x), Value::Obj(y)) if Rc::ptr_eq(x, y))
}

/// One entry of `GraphTable::build`'s explicit work stack: the traversal
/// alternates between values and frames, so both live on the same stack.
enum Pending {
    Value(Value),
    Frame(FrameRef),
}

/// A class object's three graph references, in the order its record
/// writes them: the superclass, the enclosing object, the declaring class.
/// Traversal and encoding share one definition so the two cannot drift into
/// a reference that is written but never given an id.
fn class_refs(data: &ClassData) -> impl Iterator<Item = ObjRef> + '_ {
    data.superclass
        .iter()
        .chain(data.enclosing.iter())
        .chain(data.declaring_class.iter())
        .cloned()
}

/// Whether `obj` is a class object the format names with a *tag* rather
/// than with an object-table id: a builtin, which the loaded bootstrap
/// supplies, or the module's root class, which `load` rebuilds from the
/// image's own source before the graph is decoded. Neither exists because
/// the graph says so, so neither is a record. Every other class object is.
fn is_class_tag(heap: &Heap, obj: &ObjRef, root_class: &ClassRef) -> bool {
    heap.class_builtin(obj).is_some() || Rc::ptr_eq(obj, root_class)
}

/// Every heap object (instances, strings, symbols, arrays, closures, and
/// the class objects that are neither builtin nor the module's root) and
/// every `Frame` reachable from the roots, in discovery order. A builtin
/// class, the root class and the `platform` singleton are never entered:
/// each is named by a tag of its own.
pub struct GraphTable {
    objects: Vec<ObjRef>,
    object_ids: HashMap<usize, u32>,
    frames: Vec<FrameRef>,
    frame_ids: HashMap<usize, u32>,
}

impl GraphTable {
    /// Pre-order depth-first from the roots, over an *explicit* stack
    /// rather than the Rust call stack. A live heap's object graph is as
    /// deep as the program that built it -- a Newspeak program can chain a
    /// million arrays or a million activations -- and recursing over it
    /// would overflow the stack, which aborts the process instead of
    /// failing. The order is exactly what the recursive formulation would
    /// produce (children pushed in reverse so they pop in order), which is
    /// what makes the encoding canonical.
    pub fn build(
        heap: &Heap,
        roots: &[Value],
        platform: &Value,
        root_class: &ClassRef,
    ) -> GraphTable {
        let mut table = GraphTable {
            objects: Vec::new(),
            object_ids: HashMap::new(),
            frames: Vec::new(),
            frame_ids: HashMap::new(),
        };
        let mut stack: Vec<Pending> = roots
            .iter()
            .rev()
            .map(|v| Pending::Value(v.clone()))
            .collect();
        while let Some(pending) = stack.pop() {
            match pending {
                Pending::Value(v) => table.visit_value(heap, &v, platform, root_class, &mut stack),
                Pending::Frame(f) => table.visit_frame(heap, &f, &mut stack),
            }
        }
        table
    }

    fn visit_value(
        &mut self,
        heap: &Heap,
        v: &Value,
        platform: &Value,
        root_class: &ClassRef,
        stack: &mut Vec<Pending>,
    ) {
        let Value::Obj(obj) = v else { return };
        if is_same_object(v, platform) || is_class_tag(heap, obj, root_class) {
            return;
        }
        let key = Rc::as_ptr(obj) as usize;
        if self.object_ids.contains_key(&key) {
            return;
        }
        self.object_ids
            .insert(key, u32_field("object table id", self.objects.len()));
        self.objects.push(obj.clone());
        let before = stack.len();
        {
            let record = heap.object_snapshot(obj);
            // An object's own class is a reference like every other: now
            // that a class object can be a record, reaching one only
            // through whatever values happen to point at it would leave
            // `encode_class_tag` with an id that was never assigned.
            stack.push(Pending::Value(Value::Obj(record.class.clone())));
            match &record.data {
                ObjectData::Instance(fields) | ObjectData::Array(fields) => {
                    stack.extend(fields.iter().cloned().map(Pending::Value));
                }
                ObjectData::Closure(c) => {
                    stack.push(Pending::Frame(c.captured_frame.clone()));
                    stack.push(Pending::Frame(c.home.clone()));
                }
                // A class object's three references. `metaclass` is not one
                // of them: it is a lazy cache `Interpreter::metaclass_of`
                // rebuilds on demand, and nothing hands a metaclass to
                // Newspeak code, so no value in the graph can point at one.
                ObjectData::Class(data) => {
                    stack.extend(class_refs(data).map(|c| Pending::Value(Value::Obj(c))));
                }
                ObjectData::Str(_) | ObjectData::Sym(_) => {}
            }
        }
        stack[before..].reverse();
    }

    fn visit_frame(&mut self, heap: &Heap, frame: &FrameRef, stack: &mut Vec<Pending>) {
        let key = Rc::as_ptr(frame) as usize;
        if self.frame_ids.contains_key(&key) {
            return;
        }
        self.frame_ids
            .insert(key, u32_field("frame table id", self.frames.len()));
        self.frames.push(frame.clone());
        let before = stack.len();
        {
            let f = heap.frame_snapshot(frame);
            stack.push(Pending::Value(f.receiver.clone()));
            stack.extend(f.args.iter().cloned().map(Pending::Value));
            stack.extend(f.temps.iter().cloned().map(Pending::Value));
            stack.extend(
                f.sender
                    .iter()
                    .chain(f.home.iter())
                    .cloned()
                    .map(Pending::Frame),
            );
            stack.extend(
                f.current_class
                    .iter()
                    .cloned()
                    .map(|c| Pending::Value(Value::Obj(c))),
            );
            match &f.kind {
                // A `FrameKind::Closure` frame's own closure record is
                // encoded inline, so its two frame references need ids too.
                FrameKind::Closure(c) => {
                    stack.push(Pending::Frame(c.captured_frame.clone()));
                    stack.push(Pending::Frame(c.home.clone()));
                }
                // The three class-carrying kinds name a class the same way
                // `current_class` does, so the class needs an id the same way.
                FrameKind::Initializer(c)
                | FrameKind::SuperclassClause(c)
                | FrameKind::SuperclassMessage(c) => {
                    stack.push(Pending::Value(Value::Obj(c.clone())));
                }
                FrameKind::Method(_) | FrameKind::TopLevel => {}
            }
        }
        stack[before..].reverse();
    }

    pub fn object_id(&self, obj: &ObjRef) -> u32 {
        *self
            .object_ids
            .get(&(Rc::as_ptr(obj) as usize))
            .expect("object was visited by build()")
    }

    pub fn frame_id(&self, frame: &FrameRef) -> u32 {
        *self
            .frame_ids
            .get(&(Rc::as_ptr(frame) as usize))
            .expect("frame was visited by build()")
    }

    pub fn objects(&self) -> &[ObjRef] {
        &self.objects
    }

    pub fn frames(&self) -> &[FrameRef] {
        &self.frames
    }
}

/// Everything the graph encoder resolves a reference against: the object
/// and frame tables, the `platform` singleton and the module's root class
/// (each named by a tag of its own rather than by an id), the code table's
/// ids, and the mixin table's row ids -- which is how a class object's
/// record names the declaration it applies.
pub struct EncodeContext<'a> {
    pub table: &'a GraphTable,
    pub code_ids: &'a CodeIds,
    pub platform: &'a Value,
    pub root_class: &'a ClassRef,
    /// `mixin::mixin_key` -> mixin-table row id, as `mixins::encode`
    /// assigned them.
    pub mixin_ids: &'a HashMap<usize, u32>,
    pub heap: &'a Heap,
}

/// `Builtin(kind)` (0 + kind tag), `UserRoot` (1), or `Record` (2 + object
/// id) -- the three ways a class object can exist in a loaded world.
///
/// The first two are rebuilt before the graph is: a builtin comes from the
/// bootstrap, the root class from mixin-table row 0 plus the root-class
/// section (`decode::build_root_class`).
/// Everything else is a class object the *graph* creates, and tag 2 names
/// which one by the id of its own record in the object table. That third
/// case is what nesting needs: a nested class object is the value of its
/// enclosing instance's hidden cache slot and the `.class` of every
/// instance of it, and there is one per enclosing instance, so no tag that
/// names a class only by its position in the source could tell them apart.
fn encode_class_tag(w: &mut Writer, class: &ClassRef, ctx: &EncodeContext) {
    if !ctx.heap.is_class(class) {
        panic!("internal: a class reference does not point at a class object");
    }
    let builtin = ctx.heap.class_builtin(class);
    match builtin {
        Some(kind) => {
            w.write_u8(CLS_BUILTIN);
            w.write_u8(builtin_class_tag(kind));
        }
        None if Rc::ptr_eq(class, ctx.root_class) => w.write_u8(CLS_USER_ROOT),
        None => {
            w.write_u8(CLS_RECORD);
            w.write_u32(ctx.table.object_id(class));
        }
    }
}

fn encode_optional_class(w: &mut Writer, class: Option<&ClassRef>, ctx: &EncodeContext) {
    match class {
        Some(c) => {
            w.write_bool(true);
            encode_class_tag(w, c, ctx);
        }
        None => w.write_bool(false),
    }
}

/// The general `Value` encoder: scalar tags 0-4 (`encode_scalar_value`),
/// 5 = object-table id, 6 = class tag, 7 = the `platform` singleton.
///
/// A class object always goes through tag 6, even the ones tag 6 then
/// resolves to an object-table id: one rule per kind of referent is what
/// keeps the encoding canonical.
fn encode_value(w: &mut Writer, v: &Value, ctx: &EncodeContext) {
    if encode_scalar_value(w, v) {
        return;
    }
    let Value::Obj(obj) = v else {
        unreachable!("encode_scalar_value handles every non-Obj case")
    };
    if is_same_object(v, ctx.platform) {
        w.write_u8(VAL_PLATFORM);
        return;
    }
    if ctx.heap.is_class(obj) {
        w.write_u8(VAL_CLASS_TAG);
        encode_class_tag(w, obj, ctx);
        return;
    }
    w.write_u8(VAL_OBJECT_ID);
    w.write_u32(ctx.table.object_id(obj));
}

fn encode_values(w: &mut Writer, values: &[Value], ctx: &EncodeContext) {
    w.write_u32(u32_field("value list length", values.len()));
    for v in values {
        encode_value(w, v, ctx);
    }
}

fn encode_frame_ref(w: &mut Writer, frame: Option<&FrameRef>, table: &GraphTable) {
    match frame {
        Some(f) => {
            w.write_bool(true);
            w.write_u32(table.frame_id(f));
        }
        None => w.write_bool(false),
    }
}

/// `params`, `code_id`, `captured_frame` id, `home` id. `bindings` is not
/// serialized: a compiled closure never reads one (lowering already turned
/// every binding into an instruction), so a decoded closure carries the
/// load's empty map -- see `image::LoadedImage::empty_bindings`.
fn encode_closure_record(
    w: &mut Writer,
    closure: &ClosureData,
    ctx: &EncodeContext,
    symbols: &mut symbols::Builder,
) -> Result<(), String> {
    let ClosureBody::Compiled(code) = &closure.body else {
        return Err(
            "image encoding cannot serialize a closure created by the tree-walker \
                    (ClosureBody::Ast); snapshot always runs under the bytecode engine"
                .to_string(),
        );
    };
    w.write_u16(u16_field("closure parameter count", closure.params.len()));
    for p in &closure.params {
        w.write_u32(symbols.intern(p.as_str()));
    }
    w.write_u32(ctx.code_ids.id_of(code));
    w.write_u32(ctx.table.frame_id(&closure.captured_frame));
    w.write_u32(ctx.table.frame_id(&closure.home));
    Ok(())
}

/// A class object's record: which declaration it applies, and the three
/// references that make it *this* application of it rather than another.
///
/// The `MixinApplication` itself is deliberately absent. Layout and lookup
/// follow deterministically from the mixin and the superclass's own
/// application, so `image::decode` rebuilds each one through
/// `Interpreter::apply_mixin` -- whose cache is keyed by exactly that pair,
/// which is what gives two enclosing instances back the *one* application
/// they shared before the snapshot (§6.2). Writing applications out would
/// have to reproduce that sharing by hand, and getting it wrong is silent:
/// the world still runs, and every inline cache built on it goes
/// megamorphic.
///
/// `builtin` is not written either -- a record only ever describes a user
/// class, since a builtin is a class *tag* -- and neither is `metaclass`,
/// a lazy cache `Interpreter::metaclass_of` rebuilds on the first
/// class-side send. A metaclass could not be written even if it were
/// wanted: when a class declares no class side of its own but an ancestor
/// does, `metaclass_of` applies a *synthetic* mixin that has no
/// mixin-table row, so this function's own id lookup below refuses such a
/// world -- the residual case where a snapshot is still an error rather
/// than an image.
fn encode_class_record(
    w: &mut Writer,
    data: &ClassData,
    ctx: &EncodeContext,
) -> Result<(), String> {
    let mixin = &data.application.mixin;
    let id = ctx
        .mixin_ids
        .get(&crate::mixin::mixin_key(mixin))
        .copied()
        .ok_or_else(|| {
            format!(
                "image encoding cannot serialize a class object applying mixin `{}`, \
                 which is not part of the module's declaration tree",
                mixin.name
            )
        })?;
    w.write_u32(id);
    encode_optional_class(w, data.superclass.as_ref(), ctx);
    // The enclosing object is an ordinary object, not a class, so it is
    // written the way every other object reference is -- one `Value`.
    match &data.enclosing {
        Some(enclosing) => {
            w.write_bool(true);
            encode_value(w, &Value::Obj(enclosing.clone()), ctx);
        }
        None => w.write_bool(false),
    }
    encode_optional_class(w, data.declaring_class.as_ref(), ctx);
    Ok(())
}

fn encode_object(
    w: &mut Writer,
    obj: &ObjRef,
    ctx: &EncodeContext,
    symbols: &mut symbols::Builder,
) -> Result<(), String> {
    let record = ctx.heap.object_snapshot(obj);
    encode_class_tag(w, &record.class, ctx);
    w.write_u32(record.identity_hash);
    match &record.data {
        ObjectData::Instance(fields) => {
            w.write_u8(OBJ_INSTANCE);
            encode_values(w, fields, ctx);
        }
        ObjectData::Str(s) => {
            w.write_u8(OBJ_STR);
            w.write_str(s);
        }
        ObjectData::Sym(s) => {
            w.write_u8(OBJ_SYM);
            w.write_u32(symbols.intern(s.as_str()));
        }
        ObjectData::Array(elements) => {
            w.write_u8(OBJ_ARRAY);
            encode_values(w, elements, ctx);
        }
        ObjectData::Closure(c) => {
            w.write_u8(OBJ_CLOSURE);
            encode_closure_record(w, c, ctx, symbols)?;
        }
        ObjectData::Class(data) => {
            w.write_u8(OBJ_CLASS);
            encode_class_record(w, data, ctx)?;
        }
    }
    Ok(())
}

fn encode_frame(
    w: &mut Writer,
    frame: &FrameRef,
    ctx: &EncodeContext,
    symbols: &mut symbols::Builder,
) -> Result<(), String> {
    let f = ctx.heap.frame_snapshot(frame);
    encode_value(w, &f.receiver, ctx);
    encode_values(w, &f.args, ctx);
    encode_values(w, &f.temps, ctx);
    w.write_u32(u32_field("frame pc", f.pc));
    // `pc` alone is ambiguous -- a statement index under one engine, an
    // instruction index under the other -- so the unit travels with it.
    // Only `Bytecode` is representable: an image's code table holds
    // compiled bodies and no statements, so a tree-walker activation's
    // `pc` would decode as an index into a body it never ran. Refused for
    // the same reason `encode_closure_record` refuses `ClosureBody::Ast`.
    match f.pc_kind {
        PcKind::Bytecode => w.write_u8(PC_KIND_BYTECODE),
        PcKind::TreeWalk => {
            return Err(
                "image encoding cannot serialize a tree-walker activation: its `pc` counts \
                 statements, which an image's code table does not carry"
                    .to_string(),
            )
        }
    }
    encode_frame_ref(w, f.sender.as_ref(), ctx.table);
    encode_frame_ref(w, f.home.as_ref(), ctx.table);
    encode_optional_class(w, f.current_class.as_ref(), ctx);
    match &f.kind {
        FrameKind::Method(method) => {
            w.write_u8(FRAME_METHOD);
            w.write_u32(symbols.intern(method.selector.as_str()));
        }
        FrameKind::Closure(c) => {
            w.write_u8(FRAME_CLOSURE);
            encode_closure_record(w, c, ctx, symbols)?;
        }
        FrameKind::TopLevel => w.write_u8(FRAME_TOP_LEVEL),
        FrameKind::Initializer(class) => {
            w.write_u8(FRAME_INITIALIZER);
            encode_class_tag(w, class, ctx);
        }
        FrameKind::SuperclassClause(class) => {
            w.write_u8(FRAME_SUPERCLASS_CLAUSE);
            encode_class_tag(w, class, ctx);
        }
        FrameKind::SuperclassMessage(class) => {
            w.write_u8(FRAME_SUPERCLASS_MESSAGE);
            encode_class_tag(w, class, ctx);
        }
    }
    w.write_bool(f.uncontinuable);
    Ok(())
}

/// The object table, the frame table, and the roots. Both counts come
/// first so the decoder can placeholder-allocate both tables before
/// filling either (objects and frames reference each other freely).
#[allow(clippy::too_many_arguments)]
pub fn encode_graph_section(
    w: &mut Writer,
    heap: &Heap,
    roots: &[Value],
    platform: &Value,
    code_ids: &CodeIds,
    root_class: &ClassRef,
    mixin_ids: &HashMap<usize, u32>,
    symbols: &mut symbols::Builder,
) -> Result<(), String> {
    let table = GraphTable::build(heap, roots, platform, root_class);
    let ctx = EncodeContext {
        table: &table,
        code_ids,
        platform,
        root_class,
        mixin_ids,
        heap,
    };
    w.write_u32(u32_field("object table entry count", table.objects().len()));
    w.write_u32(u32_field("frame table entry count", table.frames().len()));
    for obj in table.objects() {
        encode_object(w, obj, &ctx, symbols)?;
    }
    for frame in table.frames() {
        encode_frame(w, frame, &ctx, symbols)?;
    }
    encode_values(w, roots, &ctx);
    Ok(())
}
