//! Canonical, little-endian, self-describing byte encoding for the image
//! format -- the spec's "Image format" section. Never a memory dump.

use std::collections::HashMap;
use std::rc::Rc;

use crate::bytecode::{
    class_key, method_key, CallSite, CodeKind, CompiledCode, CompiledModule, Instr, Literal,
};
use crate::types::{BuiltinClass, ClassRef, ObjectData, Value};

pub const MAGIC: &[u8; 4] = b"NSIM";
/// Versions the whole layout *and* the compiler's codegen: any change to
/// `Instr`, to what the compiler emits, or to any section below bumps it.
/// A mismatch is refused on load, never migrated (Milestone 2 Non-Goals).
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

/// Encodes the four non-`Obj` `Value` variants (tags 0-4); returns `false`
/// for `Value::Obj`, which the object-graph encoder (Task 14) handles.
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
/// `encode_code_table`; the object/frame encoders (Task 14) use it for
/// closures' code references.
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
    }
}
