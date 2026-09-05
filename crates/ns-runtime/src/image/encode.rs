//! Canonical, little-endian, self-describing byte encoding for the image
//! format -- the spec's "Image format" section. Never a memory dump.

use std::collections::HashMap;
use std::rc::Rc;

use crate::bytecode::{
    class_key, method_key, CallSite, CodeKind, CompiledCode, CompiledModule, Instr, Literal,
};
use crate::types::{
    BuiltinClass, ClassRef, ClosureBody, ClosureData, FrameKind, FrameRef, ObjRef, ObjectData,
    SourceOrigin, Value,
};

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
/// Literal tag 9 (`UnresolvedIdent`) was added by the same reasoning, and
/// deliberately: it is a pure addition to the literal tag space made
/// before any released build could write an image, so no version-1 image
/// in anyone's hands can be misread because of it. The next such change
/// after a release bumps the version.
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

pub fn write_header(w: &mut Writer) {
    w.write_raw(MAGIC);
    w.write_u32(FORMAT_VERSION);
}

/// The module's provenance, stored so `load` can re-resolve the image's
/// source exactly as it was resolved when the snapshot was taken.
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
        Value::Nil => w.write_u8(0),
        Value::Bool(false) => w.write_u8(1),
        Value::Bool(true) => w.write_u8(2),
        Value::SmallInt(n) => {
            w.write_u8(3);
            w.write_i64(*n);
        }
        Value::Float(f) => {
            w.write_u8(4);
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
fn u32_field(what: &str, n: usize) -> u32 {
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

/// The code table: every `CompiledCode` reachable from `module` -- each
/// method, the root class's initializer, and every nested block body --
/// with nested block code *before* the code that creates it, then the root
/// list (`(selector, id)` sorted by selector, and the initializer id).
/// Deterministic: the same module always encodes to the same bytes.
pub fn encode_code_table(
    w: &mut Writer,
    module: &CompiledModule,
    root_class: &ClassRef,
) -> CodeIds {
    let mut order: Vec<Rc<CompiledCode>> = Vec::new();
    let mut ids = CodeIds {
        ids: HashMap::new(),
    };
    let mut methods: Vec<(String, Rc<CompiledCode>)> = {
        let borrowed = root_class.borrow();
        match &borrowed.data {
            ObjectData::Class(data) => data
                .methods
                .iter()
                .map(|(selector, decl)| {
                    let code = module
                        .methods
                        .get(&method_key(decl))
                        .expect("compile_module compiled every loaded method")
                        .clone();
                    (selector.as_str().to_string(), code)
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    // `ClassData.methods` is a `HashMap`; sorting is what makes the table
    // canonical rather than iteration-order dependent.
    methods.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, code) in &methods {
        collect_code(code, &mut order, &mut ids);
    }
    let initializer = module.initializers.get(&class_key(root_class)).cloned();
    if let Some(init) = &initializer {
        collect_code(init, &mut order, &mut ids);
    }
    w.write_u32(u32_field("code table entry count", order.len()));
    for code in &order {
        encode_code(w, code, &ids);
    }
    w.write_u32(u32_field("root list length", methods.len()));
    for (selector, code) in &methods {
        w.write_str(selector);
        w.write_u32(ids.id_of(code));
    }
    match &initializer {
        Some(init) => {
            w.write_bool(true);
            w.write_u32(ids.id_of(init));
        }
        None => w.write_bool(false),
    }
    ids
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

fn encode_code(w: &mut Writer, code: &CompiledCode, ids: &CodeIds) {
    w.write_u8(match code.kind {
        CodeKind::Method => 0,
        CodeKind::Block => 1,
        CodeKind::Initializer => 2,
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
        encode_instr(w, instr, ids);
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

fn encode_instr(w: &mut Writer, instr: &Instr, ids: &CodeIds) {
    match instr {
        Instr::LoadLiteral { dst, literal } => {
            w.write_u8(0);
            w.write_u16(*dst);
            encode_literal(w, literal);
        }
        Instr::LoadSelf { dst } => {
            w.write_u8(1);
            w.write_u16(*dst);
        }
        Instr::LoadBuiltinClass { dst, kind } => {
            w.write_u8(2);
            w.write_u16(*dst);
            w.write_u8(builtin_class_tag(*kind));
        }
        Instr::Move { dst, src } => {
            w.write_u8(3);
            w.write_u16(*dst);
            w.write_u16(*src);
        }
        Instr::StoreLocal { src, slot } => {
            w.write_u8(4);
            w.write_u16(*src);
            w.write_u16(*slot);
        }
        Instr::LoadOuterLocal { dst, depth, index } => {
            w.write_u8(5);
            w.write_u16(*dst);
            w.write_u16(*depth);
            w.write_u16(*index);
        }
        Instr::StoreOuterLocal { src, depth, index } => {
            w.write_u8(6);
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
            w.write_u8(7);
            w.write_u16(*depth);
            w.write_str(selector.as_str());
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
            w.write_u8(8);
            w.write_str(selector.as_str());
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
            w.write_u8(9);
            w.write_str(selector.as_str());
            encode_regs(w, arg_regs);
            w.write_u16(*dst);
            encode_call_site(w, call_site);
        }
        Instr::MakeClosure { dst, params, code } => {
            w.write_u8(10);
            w.write_u16(*dst);
            w.write_u16(u16_field("closure parameter count", params.len()));
            for p in params.iter() {
                w.write_str(p.as_str());
            }
            w.write_u32(ids.id_of(code));
        }
        Instr::MakeArray { dst, elem_regs } => {
            w.write_u8(11);
            w.write_u16(*dst);
            encode_regs(w, elem_regs);
        }
        Instr::InitSlot { src, field_index } => {
            w.write_u8(12);
            w.write_u16(*src);
            w.write_u32(*field_index);
        }
        Instr::Return { reg } => {
            w.write_u8(13);
            w.write_u16(*reg);
        }
        Instr::ImplicitReturn { reg } => {
            w.write_u8(14);
            w.write_u16(*reg);
        }
        Instr::NonLocalReturn { reg } => {
            w.write_u8(15);
            w.write_u16(*reg);
        }
        Instr::Jump { target } => {
            w.write_u8(16);
            w.write_u32(*target);
        }
        Instr::JumpIfNotBoolean { reg, target } => {
            w.write_u8(17);
            w.write_u16(*reg);
            w.write_u32(*target);
        }
        Instr::BranchIfFalse { reg, target } => {
            w.write_u8(18);
            w.write_u16(*reg);
            w.write_u32(*target);
        }
        Instr::BranchIfTrue { reg, target } => {
            w.write_u8(19);
            w.write_u16(*reg);
            w.write_u32(*target);
        }
    }
}

fn encode_literal(w: &mut Writer, literal: &Literal) {
    match literal {
        Literal::Nil => w.write_u8(0),
        Literal::Bool(b) => {
            w.write_u8(1);
            w.write_bool(*b);
        }
        Literal::SmallInt(n) => {
            w.write_u8(2);
            w.write_i64(*n);
        }
        Literal::Float(f) => {
            w.write_u8(3);
            w.write_f64(*f);
        }
        Literal::Str(s) => {
            w.write_u8(4);
            w.write_str(s);
        }
        Literal::Sym(s) => {
            w.write_u8(5);
            w.write_str(s.as_str());
        }
        Literal::Char(c) => {
            w.write_u8(6);
            w.write_u32(*c as u32);
        }
        Literal::Array(elements) => {
            w.write_u8(7);
            w.write_u32(u32_field("literal array length", elements.len()));
            for e in elements {
                encode_literal(w, e);
            }
        }
        Literal::MalformedInt(text) => {
            w.write_u8(8);
            w.write_str(text);
        }
        Literal::UnresolvedIdent(name) => {
            w.write_u8(9);
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

/// Every heap object (instances, strings, symbols, arrays, closures) and
/// every `Frame` reachable from the roots, in discovery order. Class
/// objects are never entered (they are class tags), nor is the `platform`
/// singleton (special-object tag).
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
    pub fn build(roots: &[Value], platform: &Value) -> GraphTable {
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
                Pending::Value(v) => table.visit_value(&v, platform, &mut stack),
                Pending::Frame(f) => table.visit_frame(&f, &mut stack),
            }
        }
        table
    }

    fn visit_value(&mut self, v: &Value, platform: &Value, stack: &mut Vec<Pending>) {
        let Value::Obj(obj) = v else { return };
        if is_same_object(v, platform) || matches!(obj.borrow().data, ObjectData::Class(_)) {
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
        match &obj.borrow().data {
            ObjectData::Instance(fields) | ObjectData::Array(fields) => {
                stack.extend(fields.iter().cloned().map(Pending::Value));
            }
            ObjectData::Closure(c) => {
                stack.push(Pending::Frame(c.captured_frame.clone()));
                stack.push(Pending::Frame(c.home.clone()));
            }
            ObjectData::Str(_) | ObjectData::Sym(_) | ObjectData::Class(_) => {}
        }
        stack[before..].reverse();
    }

    fn visit_frame(&mut self, frame: &FrameRef, stack: &mut Vec<Pending>) {
        let key = Rc::as_ptr(frame) as usize;
        if self.frame_ids.contains_key(&key) {
            return;
        }
        self.frame_ids
            .insert(key, u32_field("frame table id", self.frames.len()));
        self.frames.push(frame.clone());
        let before = stack.len();
        {
            let f = frame.borrow();
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
            // A `FrameKind::Closure` frame's own closure record is encoded
            // inline, so its two frame references need ids too.
            if let FrameKind::Closure(c) = &f.kind {
                stack.push(Pending::Frame(c.captured_frame.clone()));
                stack.push(Pending::Frame(c.home.clone()));
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

/// `Builtin(kind)` (0 + kind tag) or `UserRoot` (1) -- a class is never
/// structurally serialized (spec: "Root/special-objects table"). `UserRoot`
/// is unambiguous because a loaded world has exactly one user-defined
/// class: `Interpreter::load_module` does not load nested classes (see its
/// "Milestone 1 known gaps" note), so the only non-builtin class object any
/// value can point at is the module's root class.
///
/// That invariant is load-bearing and this function cannot check it: it is
/// handed a `ClassRef` with no root class to compare against, so a second
/// user-defined class would silently encode as `UserRoot` and decode as the
/// root -- a wrong class, not a decode error. The milestone that loads
/// nested classes must replace tag 1 with something that names *which*
/// user class (a path, or an index into a class table) and bump
/// `FORMAT_VERSION`; threading a root class through for enforcement now
/// would only turn a currently-impossible case into a check.
pub fn encode_class_tag(w: &mut Writer, class: &ClassRef) {
    let builtin = match &class.borrow().data {
        ObjectData::Class(data) => data.builtin,
        _ => panic!("internal: a class reference does not point at a class object"),
    };
    match builtin {
        Some(kind) => {
            w.write_u8(0);
            w.write_u8(builtin_class_tag(kind));
        }
        None => w.write_u8(1),
    }
}

/// The general `Value` encoder: scalar tags 0-4 (`encode_scalar_value`),
/// 5 = object-table id, 6 = class tag, 7 = the `platform` singleton.
pub fn encode_value(w: &mut Writer, v: &Value, table: &GraphTable, platform: &Value) {
    if encode_scalar_value(w, v) {
        return;
    }
    let Value::Obj(obj) = v else {
        unreachable!("encode_scalar_value handles every non-Obj case")
    };
    if is_same_object(v, platform) {
        w.write_u8(7);
        return;
    }
    if matches!(obj.borrow().data, ObjectData::Class(_)) {
        w.write_u8(6);
        encode_class_tag(w, obj);
        return;
    }
    w.write_u8(5);
    w.write_u32(table.object_id(obj));
}

fn encode_values(w: &mut Writer, values: &[Value], table: &GraphTable, platform: &Value) {
    w.write_u32(u32_field("value list length", values.len()));
    for v in values {
        encode_value(w, v, table, platform);
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
/// serialized (the reloaded module's map is used on load).
fn encode_closure_record(
    w: &mut Writer,
    closure: &ClosureData,
    table: &GraphTable,
    code_ids: &CodeIds,
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
        w.write_str(p.as_str());
    }
    w.write_u32(code_ids.id_of(code));
    w.write_u32(table.frame_id(&closure.captured_frame));
    w.write_u32(table.frame_id(&closure.home));
    Ok(())
}

fn encode_object(
    w: &mut Writer,
    obj: &ObjRef,
    table: &GraphTable,
    code_ids: &CodeIds,
    platform: &Value,
) -> Result<(), String> {
    let borrowed = obj.borrow();
    encode_class_tag(w, &borrowed.class);
    w.write_u32(borrowed.identity_hash);
    match &borrowed.data {
        ObjectData::Instance(fields) => {
            w.write_u8(0);
            encode_values(w, fields, table, platform);
        }
        ObjectData::Str(s) => {
            w.write_u8(1);
            w.write_str(s);
        }
        ObjectData::Sym(s) => {
            w.write_u8(2);
            w.write_str(s.as_str());
        }
        ObjectData::Array(elements) => {
            w.write_u8(3);
            encode_values(w, elements, table, platform);
        }
        ObjectData::Closure(c) => {
            w.write_u8(4);
            encode_closure_record(w, c, table, code_ids)?;
        }
        ObjectData::Class(_) => {
            unreachable!("class objects are never entered in the object table")
        }
    }
    Ok(())
}

fn encode_frame(
    w: &mut Writer,
    frame: &FrameRef,
    table: &GraphTable,
    code_ids: &CodeIds,
    platform: &Value,
) -> Result<(), String> {
    let f = frame.borrow();
    encode_value(w, &f.receiver, table, platform);
    encode_values(w, &f.args, table, platform);
    encode_values(w, &f.temps, table, platform);
    w.write_u32(u32_field("frame pc", f.pc));
    encode_frame_ref(w, f.sender.as_ref(), table);
    encode_frame_ref(w, f.home.as_ref(), table);
    match &f.current_class {
        Some(c) => {
            w.write_bool(true);
            encode_class_tag(w, c);
        }
        None => w.write_bool(false),
    }
    match &f.kind {
        FrameKind::Method(decl) => {
            w.write_u8(0);
            w.write_str(&crate::interpreter::instantiate::method_name(&decl.pattern));
        }
        FrameKind::Closure(c) => {
            w.write_u8(1);
            encode_closure_record(w, c, table, code_ids)?;
        }
        FrameKind::TopLevel => w.write_u8(2),
        FrameKind::Initializer(class) => {
            w.write_u8(3);
            encode_class_tag(w, class);
        }
    }
    w.write_bool(f.uncontinuable);
    Ok(())
}

/// The object table, the frame table, and the roots. Both counts come
/// first so the decoder can placeholder-allocate both tables before
/// filling either (objects and frames reference each other freely).
pub fn encode_graph_section(
    w: &mut Writer,
    roots: &[Value],
    platform: &Value,
    code_ids: &CodeIds,
) -> Result<(), String> {
    let table = GraphTable::build(roots, platform);
    w.write_u32(u32_field("object table entry count", table.objects().len()));
    w.write_u32(u32_field("frame table entry count", table.frames().len()));
    for obj in table.objects() {
        encode_object(w, obj, &table, code_ids, platform)?;
    }
    for frame in table.frames() {
        encode_frame(w, frame, &table, code_ids, platform)?;
    }
    encode_values(w, roots, &table, platform);
    Ok(())
}
