//! Canonical, little-endian, self-describing byte encoding for the image
//! format -- the spec's "Image format" section. Never a memory dump.

use crate::types::Value;

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

    /// `u32` byte-count prefix, then the raw bytes.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.write_u32(bytes.len() as u32);
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
