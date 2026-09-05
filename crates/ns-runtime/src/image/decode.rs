//! Canonical decoding, the inverse of `encode.rs`. A malformed byte stream
//! is a `DecodeError`, never a panic.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::Span;

use crate::bytecode::{
    class_key, method_key, CallSite, CodeKind, CompiledCode, CompiledModule, Instr, Literal,
};
use crate::types::{Binding, BuiltinClass, ClassRef, ObjectData, Value};
use crate::Symbol;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    UnexpectedEof,
    BadMagic,
    VersionMismatch {
        found: u32,
        expected: u32,
    },
    InvalidTag {
        what: &'static str,
        tag: u8,
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
    BadCodeId(u32),
    BadObjectId(u32),
    BadFrameId(u32),
    /// The image's root object is not a class, so it has no methods to
    /// install a code table against.
    NotAClass,
    /// The image's root list names a method the reloaded source dropped.
    UnknownSelector(String),
    /// The mirror image of `UnknownSelector`: the reloaded source defines a
    /// method the image was never compiled with, so the installed module
    /// would have a hole the engine hook cannot dispatch through.
    MissingSelector(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::UnexpectedEof => write!(f, "unexpected end of image"),
            DecodeError::BadMagic => write!(f, "not an ns-next image (bad magic)"),
            DecodeError::VersionMismatch { found, expected } => {
                write!(f, "image format version {found} is not supported (this build reads version {expected})")
            }
            DecodeError::InvalidTag { what, tag } => write!(f, "invalid {what} tag {tag}"),
            DecodeError::InvalidUtf8 => write!(f, "invalid UTF-8 in image string"),
            DecodeError::InvalidSpan { start, end } => {
                write!(f, "source span {start}..{end} ends before it starts")
            }
            DecodeError::NestingTooDeep { what, limit } => {
                write!(f, "{what} in image nests deeper than {limit} levels")
            }
            DecodeError::BadCodeId(id) => write!(f, "code id {id} out of range"),
            DecodeError::BadObjectId(id) => write!(f, "object id {id} out of range"),
            DecodeError::BadFrameId(id) => write!(f, "frame id {id} out of range"),
            DecodeError::NotAClass => {
                write!(f, "the image's root object is not a class")
            }
            DecodeError::UnknownSelector(s) => write!(
                f,
                "image refers to method `{s}`, which the reloaded source does not define"
            ),
            DecodeError::MissingSelector(s) => write!(
                f,
                "the reloaded source defines method `{s}`, which the image has no compiled code for"
            ),
        }
    }
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, pos: 0 }
    }

    /// Bytes not yet consumed. Every item of a counted section costs at
    /// least one byte, so this bounds how many can really follow -- see
    /// `reserve_for`.
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn read_raw(&mut self, len: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(DecodeError::UnexpectedEof)?;
        if end > self.buf.len() {
            return Err(DecodeError::UnexpectedEof);
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn read_u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.read_raw(1)?[0])
    }

    pub fn read_bool(&mut self) -> Result<bool, DecodeError> {
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
        let len = self.read_u32()? as usize;
        Ok(self.read_raw(len)?.to_vec())
    }

    pub fn read_str(&mut self) -> Result<String, DecodeError> {
        String::from_utf8(self.read_bytes()?).map_err(|_| DecodeError::InvalidUtf8)
    }
}

pub fn read_header(r: &mut Reader) -> Result<(), DecodeError> {
    let magic = r.read_raw(4)?;
    if magic != super::encode::MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let version = r.read_u32()?;
    if version != super::encode::FORMAT_VERSION {
        return Err(DecodeError::VersionMismatch {
            found: version,
            expected: super::encode::FORMAT_VERSION,
        });
    }
    Ok(())
}

pub fn decode_scalar_value(r: &mut Reader) -> Result<Value, DecodeError> {
    match r.read_u8()? {
        0 => Ok(Value::Nil),
        1 => Ok(Value::Bool(false)),
        2 => Ok(Value::Bool(true)),
        3 => Ok(Value::SmallInt(r.read_i64()?)),
        4 => Ok(Value::Float(r.read_f64()?)),
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
        6 => BuiltinClass::Str,
        7 => BuiltinClass::Sym,
        8 => BuiltinClass::Character,
        9 => BuiltinClass::Array,
        10 => BuiltinClass::OrderedCollection,
        11 => BuiltinClass::Nil,
        12 => BuiltinClass::Block,
        13 => BuiltinClass::Platform,
        _ => return None,
    })
}

pub fn read_builtin_class(r: &mut Reader) -> Result<BuiltinClass, DecodeError> {
    let tag = r.read_u8()?;
    builtin_class_from_tag(tag).ok_or(DecodeError::InvalidTag {
        what: "builtin class",
        tag,
    })
}

/// The decoded code table before it is bound to a reloaded class.
#[derive(Debug, PartialEq)]
pub struct DecodedCodeTable {
    pub codes: Vec<Rc<CompiledCode>>,
    /// `(selector, code id)` per method of the root class, sorted by
    /// selector exactly as encoded.
    pub methods: Vec<(String, u32)>,
    pub initializer: Option<u32>,
}

pub fn decode_code_table(r: &mut Reader) -> Result<DecodedCodeTable, DecodeError> {
    let count = r.read_u32()? as usize;
    let mut codes: Vec<Rc<CompiledCode>> = reserve_for(count, r);
    for _ in 0..count {
        let code = decode_code(r, &codes)?;
        codes.push(Rc::new(code));
    }
    let method_count = r.read_u32()? as usize;
    let mut methods = reserve_for(method_count, r);
    for _ in 0..method_count {
        let selector = r.read_str()?;
        let id = r.read_u32()?;
        check_code_id(id, &codes)?;
        methods.push((selector, id));
    }
    let initializer = if r.read_bool()? {
        let id = r.read_u32()?;
        check_code_id(id, &codes)?;
        Some(id)
    } else {
        None
    };
    Ok(DecodedCodeTable {
        codes,
        methods,
        initializer,
    })
}

/// `Vec::with_capacity` for a count read from an untrusted stream: every
/// item costs at least one byte, so the bytes still unread are an upper
/// bound on how many can really follow. Without the clamp a corrupt count
/// prefix would reserve gigabytes and abort the process before the read
/// could fail with `UnexpectedEof`.
fn reserve_for<T>(count: usize, r: &Reader) -> Vec<T> {
    Vec::with_capacity(count.min(r.remaining()))
}

fn check_code_id(id: u32, codes: &[Rc<CompiledCode>]) -> Result<(), DecodeError> {
    if (id as usize) < codes.len() {
        Ok(())
    } else {
        Err(DecodeError::BadCodeId(id))
    }
}

/// Binds a decoded table to the *reloaded* root class: methods re-keyed by
/// selector to the reloaded `Rc<MethodDecl>`s, the initializer to the
/// reloaded class. The image's bytecode is what runs after a load.
pub fn install_code_table(
    table: &DecodedCodeTable,
    root_class: &ClassRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> Result<CompiledModule, DecodeError> {
    let mut module = CompiledModule {
        methods: HashMap::new(),
        initializers: HashMap::new(),
        bindings: bindings.clone(),
    };
    let borrowed = root_class.borrow();
    let ObjectData::Class(data) = &borrowed.data else {
        return Err(DecodeError::NotAClass);
    };
    for (selector, id) in &table.methods {
        let decl = data
            .methods
            .get(&Symbol::intern(selector))
            .ok_or_else(|| DecodeError::UnknownSelector(selector.clone()))?;
        module
            .methods
            .insert(method_key(decl), code_at(table, *id)?);
    }
    // The other direction: a method the reloaded source defines but the
    // image has no code for would leave `module.methods` with a hole, and
    // dispatching it panics in the engine hook. That is the ordinary
    // "source changed since the snapshot" path, so it is a decode error
    // here rather than a crash later. Sorted, so the reported selector does
    // not depend on `ClassData.methods`' `HashMap` order.
    let mut missing: Vec<&str> = data
        .methods
        .iter()
        .filter(|(_, decl)| !module.methods.contains_key(&method_key(decl)))
        .map(|(selector, _)| selector.as_str())
        .collect();
    missing.sort_unstable();
    if let Some(selector) = missing.first() {
        return Err(DecodeError::MissingSelector((*selector).to_string()));
    }
    if let Some(id) = table.initializer {
        module
            .initializers
            .insert(class_key(root_class), code_at(table, id)?);
    }
    Ok(module)
}

/// `decode_code_table` validates every id it reads, but `DecodedCodeTable`
/// is a plain public struct: an id is re-checked before it indexes.
fn code_at(table: &DecodedCodeTable, id: u32) -> Result<Rc<CompiledCode>, DecodeError> {
    table
        .codes
        .get(id as usize)
        .cloned()
        .ok_or(DecodeError::BadCodeId(id))
}

fn decode_code(r: &mut Reader, codes: &[Rc<CompiledCode>]) -> Result<CompiledCode, DecodeError> {
    let kind = match r.read_u8()? {
        0 => CodeKind::Method,
        1 => CodeKind::Block,
        2 => CodeKind::Initializer,
        tag => {
            return Err(DecodeError::InvalidTag {
                what: "code kind",
                tag,
            })
        }
    };
    let num_args = r.read_u16()?;
    let num_registers = r.read_u16()?;
    let start = r.read_u32()?;
    let end = r.read_u32()?;
    if start > end {
        return Err(DecodeError::InvalidSpan { start, end });
    }
    let source = Span::new(start, end);
    let pc_count = r.read_u32()? as usize;
    let mut statement_pc = reserve_for(pc_count, r);
    for _ in 0..pc_count {
        let stmt = r.read_u32()? as usize;
        let range_start = r.read_u32()? as usize;
        let range_end = r.read_u32()? as usize;
        statement_pc.push((stmt, range_start..range_end));
    }
    let site_count = r.read_u32()? as usize;
    let mut call_sites = reserve_for(site_count, r);
    for _ in 0..site_count {
        call_sites.push(decode_call_site(r)?);
    }
    let instr_count = r.read_u32()? as usize;
    let mut instrs = reserve_for(instr_count, r);
    for _ in 0..instr_count {
        instrs.push(decode_instr(r, codes)?);
    }
    Ok(CompiledCode {
        kind,
        instrs,
        num_args,
        num_registers,
        statement_pc,
        call_sites,
        source,
    })
}

fn decode_call_site(r: &mut Reader) -> Result<CallSite, DecodeError> {
    Ok(CallSite {
        call_site_id: r.read_u16()?,
    })
}

fn decode_regs(r: &mut Reader) -> Result<Vec<u16>, DecodeError> {
    let n = r.read_u16()? as usize;
    let mut regs = reserve_for(n, r);
    for _ in 0..n {
        regs.push(r.read_u16()?);
    }
    Ok(regs)
}

fn decode_symbol(r: &mut Reader) -> Result<Symbol, DecodeError> {
    Ok(Symbol::intern(&r.read_str()?))
}

fn decode_instr(r: &mut Reader, codes: &[Rc<CompiledCode>]) -> Result<Instr, DecodeError> {
    Ok(match r.read_u8()? {
        0 => {
            let dst = r.read_u16()?;
            Instr::LoadLiteral {
                dst,
                literal: decode_literal(r, 0)?,
            }
        }
        1 => Instr::LoadSelf { dst: r.read_u16()? },
        2 => {
            let dst = r.read_u16()?;
            Instr::LoadBuiltinClass {
                dst,
                kind: read_builtin_class(r)?,
            }
        }
        3 => {
            let dst = r.read_u16()?;
            Instr::Move {
                dst,
                src: r.read_u16()?,
            }
        }
        4 => {
            let src = r.read_u16()?;
            Instr::StoreLocal {
                src,
                slot: r.read_u16()?,
            }
        }
        5 => {
            let dst = r.read_u16()?;
            let depth = r.read_u16()?;
            Instr::LoadOuterLocal {
                dst,
                depth,
                index: r.read_u16()?,
            }
        }
        6 => {
            let src = r.read_u16()?;
            let depth = r.read_u16()?;
            Instr::StoreOuterLocal {
                src,
                depth,
                index: r.read_u16()?,
            }
        }
        7 => {
            let depth = r.read_u16()?;
            let selector = decode_symbol(r)?;
            let arg_regs = decode_regs(r)?;
            let dst = r.read_u16()?;
            Instr::SendLexical {
                depth,
                selector,
                arg_regs,
                dst,
                call_site: decode_call_site(r)?,
            }
        }
        8 => {
            let selector = decode_symbol(r)?;
            let receiver_reg = r.read_u16()?;
            let arg_regs = decode_regs(r)?;
            let dst = r.read_u16()?;
            Instr::SendPublic {
                selector,
                receiver_reg,
                arg_regs,
                dst,
                call_site: decode_call_site(r)?,
            }
        }
        9 => {
            let selector = decode_symbol(r)?;
            let arg_regs = decode_regs(r)?;
            let dst = r.read_u16()?;
            Instr::SendSuper {
                selector,
                arg_regs,
                dst,
                call_site: decode_call_site(r)?,
            }
        }
        10 => {
            let dst = r.read_u16()?;
            let n = r.read_u16()? as usize;
            let mut params = reserve_for(n, r);
            for _ in 0..n {
                params.push(decode_symbol(r)?);
            }
            let id = r.read_u32()?;
            // Post-order guarantees a block's code precedes its parent's,
            // so `codes` already holds it -- a forward or out-of-range id
            // is corruption.
            check_code_id(id, codes)?;
            Instr::MakeClosure {
                dst,
                params: Rc::new(params),
                code: codes[id as usize].clone(),
            }
        }
        11 => {
            let dst = r.read_u16()?;
            Instr::MakeArray {
                dst,
                elem_regs: decode_regs(r)?,
            }
        }
        12 => {
            let src = r.read_u16()?;
            Instr::InitSlot {
                src,
                field_index: r.read_u32()?,
            }
        }
        13 => Instr::Return { reg: r.read_u16()? },
        14 => Instr::ImplicitReturn { reg: r.read_u16()? },
        15 => Instr::NonLocalReturn { reg: r.read_u16()? },
        16 => Instr::Jump {
            target: r.read_u32()?,
        },
        17 => {
            let reg = r.read_u16()?;
            Instr::JumpIfNotBoolean {
                reg,
                target: r.read_u32()?,
            }
        }
        18 => {
            let reg = r.read_u16()?;
            Instr::BranchIfFalse {
                reg,
                target: r.read_u32()?,
            }
        }
        19 => {
            let reg = r.read_u16()?;
            Instr::BranchIfTrue {
                reg,
                target: r.read_u32()?,
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
/// `parser::MAX_DEPTH` is private to `ns-syntax`, and this milestone leaves
/// that crate untouched, so the number is restated here rather than
/// imported -- it must track `crates/ns-syntax/src/parser/mod.rs`'s
/// `MAX_DEPTH`. `the_deepest_literal_array_the_parser_accepts_survives_a_round_trip`
/// in `tests/image.rs` discovers the parser's real bound at run time instead
/// of assuming it, so the two cannot drift apart silently.
const MAX_LITERAL_DEPTH: usize = 128;

fn decode_literal(r: &mut Reader, depth: usize) -> Result<Literal, DecodeError> {
    if depth > MAX_LITERAL_DEPTH {
        return Err(DecodeError::NestingTooDeep {
            what: "literal array",
            limit: MAX_LITERAL_DEPTH,
        });
    }
    Ok(match r.read_u8()? {
        0 => Literal::Nil,
        1 => Literal::Bool(r.read_bool()?),
        2 => Literal::SmallInt(r.read_i64()?),
        3 => Literal::Float(r.read_f64()?),
        4 => Literal::Str(r.read_str()?),
        5 => Literal::Sym(decode_symbol(r)?),
        6 => {
            let code = r.read_u32()?;
            Literal::Char(char::from_u32(code).ok_or(DecodeError::InvalidTag {
                what: "char",
                tag: 6,
            })?)
        }
        7 => {
            let n = r.read_u32()? as usize;
            let mut elements = reserve_for(n, r);
            for _ in 0..n {
                elements.push(decode_literal(r, depth + 1)?);
            }
            Literal::Array(elements)
        }
        8 => Literal::MalformedInt(r.read_str()?),
        tag => {
            return Err(DecodeError::InvalidTag {
                what: "literal",
                tag,
            })
        }
    })
}
