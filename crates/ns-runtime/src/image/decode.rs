//! Canonical decoding, the inverse of `encode.rs`. A malformed byte stream
//! is a `DecodeError`, never a panic.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::bytecode::{
    class_key, method_key, CallSite, CodeKind, CompiledCode, CompiledModule, Instr, Literal,
};
use crate::interpreter::instantiate;
use crate::types::{
    Binding, BuiltinClass, ClassRef, ClosureBody, ClosureData, Frame, FrameKind, FrameRef, ObjRef,
    Object, ObjectData, SourceOrigin, Value,
};
use crate::Interpreter;
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
    /// A closure record's parameter list and the code it names disagree
    /// about how many arguments the closure takes. The two are separate
    /// fields on the wire, so only a decode-time reconciliation can catch
    /// a crafted mismatch -- and it must, because the two halves are read
    /// by different parts of `invoke_closure`: it checks the *call's*
    /// arity against `params.len()` but sizes the register file from the
    /// *code* (`num_registers - num_args`). Disagreement leaves
    /// `eval::read_local` computing `temps[i - args.len()]` past the end
    /// of `temps` -- an unchecked index panic from a version-matching
    /// image -- or, the other way round, silently misaddressing temps as
    /// arguments.
    ClosureArityMismatch {
        params: usize,
        num_args: u16,
    },
    /// The image's root object is not a class, so it has no methods to
    /// install a code table against.
    NotAClass,
    /// The image's root list names a method the reloaded source dropped.
    UnknownSelector(String),
    /// The mirror image of `UnknownSelector`: the reloaded source defines a
    /// method the image was never compiled with, so the installed module
    /// would have a hole the engine hook cannot dispatch through.
    MissingSelector(String),
    /// `MissingSelector` for the instance initializer: the reloaded root
    /// class declares slots or `initExpr`s, so instantiating it runs
    /// compiled initializer code -- which this image does not carry.
    /// `run_own_initializer` panics on that invariant rather than failing,
    /// and the class it panics for is the one the image's own source
    /// declares, so only refusing here keeps a crafted image from aborting
    /// the process at the first `X new`.
    MissingInitializer,
    /// A compiled body installed where a body of another kind belongs, or
    /// named as a closure's body when it is not a block body. The kinds are
    /// not interchangeable: a block body runs in a `FrameKind::Closure`
    /// activation and a method's or initializer's does not, which is what
    /// `frame_at_depth` (and so `verify_code_table`'s outer-local bound)
    /// walks.
    WrongCodeKind {
        expected: &'static str,
    },
    /// A method's -- or the instance initializer's -- compiled body takes a
    /// different number of arguments than the reloaded source declares. The
    /// frame's `args` come from the send (or the factory), its `temps` from
    /// the body's `num_args`; a disagreement leaves `read_local` indexing
    /// `temps` past its end.
    CodeArityMismatch {
        what: String,
        params: usize,
        num_args: u16,
    },
    /// A body with no instructions, or one whose last instruction falls
    /// through: `run_code` fetches `instrs[pc]` after every fall-through,
    /// so either runs off the end of the vector.
    UnterminatedBody,
    /// A register operand outside its own body's register file.
    RegisterOutOfRange {
        reg: u16,
        num_registers: u16,
    },
    /// An outer-local operand outside the register file of the body it
    /// addresses -- the one `depth` closure hops out.
    OuterLocalOutOfRange {
        depth: u16,
        index: u16,
        num_registers: u16,
    },
    /// A jump target that is not an instruction index of its own body.
    JumpTargetOutOfRange {
        target: u32,
        instrs: usize,
    },
    /// More arguments than registers: every engine hook sizes a frame's
    /// temps as `num_registers - num_args`, which underflows here.
    ArgsExceedRegisters {
        num_args: u16,
        num_registers: u16,
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
    /// `args`/`temps` out of range.
    FrameRegisterFile {
        args: usize,
        locals: usize,
        num_args: u16,
        num_registers: u16,
    },
    /// A decoded closure's captured frame is not an activation of the body
    /// its own body is lexically nested in -- the link every
    /// `LoadOuterLocal`/`StoreOuterLocal` bound rests on.
    CapturedFrameMismatch,
    /// An object's identity hash is `u32::MAX`, so there is no hash a
    /// loaded world could allocate *next*:
    /// `Interpreter::advance_identity_hash_past` would have to compute
    /// `u32::MAX + 1` (an overflow panic in this workspace's dev profile,
    /// and a wrap to 0 -- colliding with the loaded objects it exists to
    /// avoid -- in release), and even saturating it would only move the
    /// overflow to `alloc_identity_hash`'s own `+= 1`. Refusing the image
    /// is the only answer that neither panics nor collides.
    IdentityHashExhausted,
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
            DecodeError::ClosureArityMismatch { params, num_args } => write!(
                f,
                "closure record declares {params} parameters but the code it names takes {num_args}"
            ),
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
            DecodeError::MissingInitializer => write!(
                f,
                "the reloaded source's root class needs an instance initializer, which the image has no compiled code for"
            ),
            DecodeError::WrongCodeKind { expected } => {
                write!(f, "image code is not a {expected} body")
            }
            DecodeError::CodeArityMismatch {
                what,
                params,
                num_args,
            } => write!(
                f,
                "the image's compiled {what} takes {num_args} argument(s), but the reloaded source declares {params}"
            ),
            DecodeError::UnterminatedBody => write!(
                f,
                "image code body does not end in a return or a jump, so it would run past its last instruction"
            ),
            DecodeError::RegisterOutOfRange {
                reg,
                num_registers,
            } => write!(
                f,
                "register r{reg} is outside a body with {num_registers} register(s)"
            ),
            DecodeError::OuterLocalOutOfRange {
                depth,
                index,
                num_registers,
            } => write!(
                f,
                "outer local r{index} at depth {depth} is outside the enclosing body's {num_registers} register(s)"
            ),
            DecodeError::JumpTargetOutOfRange { target, instrs } => write!(
                f,
                "jump target {target} is not an instruction index in a body of {instrs} instruction(s)"
            ),
            DecodeError::ArgsExceedRegisters {
                num_args,
                num_registers,
            } => write!(
                f,
                "image code takes {num_args} argument(s) but declares only {num_registers} register(s)"
            ),
            DecodeError::CodeNotInTable => {
                write!(f, "image code refers to a body outside its code table")
            }
            DecodeError::SharedBlockBody => write!(
                f,
                "one block body in the image is created by more than one MakeClosure"
            ),
            DecodeError::FrameRegisterFile {
                args,
                locals,
                num_args,
                num_registers,
            } => write!(
                f,
                "a decoded frame holds {args} argument(s) and {locals} local(s), but the code that runs in it takes {num_args} argument(s) in {num_registers} register(s)"
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

/// The inverse of `encode::encode_source_origin`. `load` re-resolves the
/// image's stored source against the origin it decodes here, so a loaded
/// world reports the same provenance the snapshotted one did.
pub fn decode_source_origin(r: &mut Reader) -> Result<SourceOrigin, DecodeError> {
    match r.read_u8()? {
        0 => Ok(SourceOrigin::File(PathBuf::from(r.read_str()?))),
        tag => Err(DecodeError::InvalidTag {
            what: "source origin",
            tag,
        }),
    }
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
            .application
            .mixin
            .methods
            .get(&Symbol::intern(selector))
            .ok_or_else(|| DecodeError::UnknownSelector(selector.clone()))?;
        let code = code_at(table, *id)?;
        // The frame an installed body runs in is built by the shared
        // orchestration, not by the body: `invoke_method` fills `args` from
        // the send (so, from the selector's arity) and sizes `temps` from
        // the body's `num_args`. A body whose arity disagrees with the
        // reloaded declaration therefore leaves `read_local` splitting a
        // frame it does not fit -- reading `temps` past its end for a body
        // that claims too many arguments. Only this function sees both.
        check_code_arity(
            &code,
            CodeKind::Method,
            format!("method `{selector}`"),
            instantiate::pattern_param_count(&decl.pattern),
        )?;
        module.methods.insert(method_key(decl), code);
    }
    // The other direction: a method the reloaded source defines but the
    // image has no code for would leave `module.methods` with a hole, and
    // dispatching it panics in the engine hook. That is the ordinary
    // "source changed since the snapshot" path, so it is a decode error
    // here rather than a crash later. Sorted, so the reported selector does
    // not depend on the mixin's method `HashMap` order.
    let mut missing: Vec<&str> = data
        .application
        .mixin
        .methods
        .iter()
        .filter(|(_, decl)| !module.methods.contains_key(&method_key(decl)))
        .map(|(selector, _)| selector.as_str())
        .collect();
    missing.sort_unstable();
    if let Some(selector) = missing.first() {
        return Err(DecodeError::MissingSelector((*selector).to_string()));
    }
    match table.initializer {
        Some(id) => {
            let code = code_at(table, id)?;
            // The same reconciliation for the initializer, whose `num_args`
            // is the class's factory parameter count
            // (`compile_initializer`) and whose frame's `args` are the
            // factory arguments `instantiate` checked against that same
            // pattern.
            check_code_arity(
                &code,
                CodeKind::Initializer,
                "instance initializer".to_string(),
                data.application
                    .mixin
                    .factory
                    .as_deref()
                    .map(instantiate::pattern_param_count)
                    .unwrap_or(0),
            )?;
            module.initializers.insert(class_key(root_class), code);
        }
        // And the initializer's `MissingSelector`: a class with slots or
        // `initExpr`s runs compiled initializer code on every `X new`, and
        // `run_own_initializer` *panics* when the module has none. The
        // reloaded source decides whether the class needs it, and the image
        // decides whether it carries it -- two halves only this function
        // sees, exactly as for the method table above.
        None if instantiate::class_has_initializer_code(root_class) => {
            return Err(DecodeError::MissingInitializer)
        }
        None => {}
    }
    Ok(module)
}

/// One installed body against the reloaded declaration it is installed for:
/// the same kind, and the same number of arguments.
fn check_code_arity(
    code: &CompiledCode,
    kind: CodeKind,
    what: String,
    params: usize,
) -> Result<(), DecodeError> {
    if code.kind != kind {
        return Err(DecodeError::WrongCodeKind {
            expected: match kind {
                CodeKind::Method => "method",
                CodeKind::Block => "block",
                CodeKind::Initializer => "initializer",
            },
        });
    }
    if code.num_args as usize != params {
        return Err(DecodeError::CodeArityMismatch {
            what,
            params,
            num_args: code.num_args,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Structural verification of a decoded code table.
//
// Decoding is byte-safe: every id, tag and length is checked as it is read.
// It does not make the result *coherent*. `vm::run_code` indexes
// `code.instrs[pc]` directly, and `eval::read_local`/`write_local` index a
// frame's `args`/`temps` directly, so a jump target, a register operand or
// an arity that decoded cleanly can still panic the process the first time
// the body runs. Everything below is a bounded, linear pass over the flat,
// topologically ordered table -- no traversal, no recursion -- and reports
// a `DecodeError` rather than leaving the VM to trip over it.
// ---------------------------------------------------------------------------

/// `Rc` pointer -> code-table index, the inverse of `encode::CodeIds`.
fn code_index(table: &DecodedCodeTable) -> HashMap<usize, usize> {
    table
        .codes
        .iter()
        .enumerate()
        .map(|(i, code)| (Rc::as_ptr(code) as usize, i))
        .collect()
}

/// For each body, the body whose `MakeClosure` creates it -- the lexical
/// parent, and so (`vm::run_code`'s `MakeClosure` arm sets
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
    for (i, code) in table.codes.iter().enumerate() {
        for instr in &code.instrs {
            let Instr::MakeClosure {
                params,
                code: block,
                ..
            } = instr
            else {
                continue;
            };
            let id = *index_of
                .get(&(Rc::as_ptr(block) as usize))
                .ok_or(DecodeError::CodeNotInTable)?;
            if block.kind != CodeKind::Block {
                return Err(DecodeError::WrongCodeKind { expected: "block" });
            }
            // The `MakeClosure` mirror of `decode_closure_record`'s check:
            // `invoke_closure` validates the call against `params.len()`
            // but sizes the register file from the body's `num_args`.
            if params.len() != block.num_args as usize {
                return Err(DecodeError::ClosureArityMismatch {
                    params: params.len(),
                    num_args: block.num_args,
                });
            }
            if parents[id].is_some() {
                return Err(DecodeError::SharedBlockBody);
            }
            parents[id] = Some(i);
        }
    }
    Ok(parents)
}

/// Every decoded body checked against what `vm::run_code` will do with it:
/// its arity fits its register file, it cannot run past its last
/// instruction, every jump target is one of its own instructions, every
/// register operand is inside its own register file, and every outer local
/// is inside the register file of the body it addresses.
pub fn verify_code_table(table: &DecodedCodeTable) -> Result<(), DecodeError> {
    let parents = lexical_parents(table)?;
    for (i, code) in table.codes.iter().enumerate() {
        verify_body(code)?;
        verify_outer_locals(table, &parents, i, code)?;
    }
    Ok(())
}

fn verify_body(code: &CompiledCode) -> Result<(), DecodeError> {
    // Every engine hook sizes a frame's temps as `num_registers -
    // num_args`, a `u16` subtraction.
    if code.num_args > code.num_registers {
        return Err(DecodeError::ArgsExceedRegisters {
            num_args: code.num_args,
            num_registers: code.num_registers,
        });
    }
    // `run_code` fetches `instrs[pc]` at the top of the loop and again
    // after every fall-through, so an empty body -- or one whose last
    // instruction falls through -- indexes past the end of the vector.
    match code.instrs.last() {
        Some(
            Instr::Return { .. }
            | Instr::ImplicitReturn { .. }
            | Instr::NonLocalReturn { .. }
            | Instr::Jump { .. },
        ) => {}
        _ => return Err(DecodeError::UnterminatedBody),
    }
    for instr in &code.instrs {
        for reg in own_registers(instr) {
            if reg >= code.num_registers {
                return Err(DecodeError::RegisterOutOfRange {
                    reg,
                    num_registers: code.num_registers,
                });
            }
        }
        if let Some(target) = jump_target(instr) {
            if target as usize >= code.instrs.len() {
                return Err(DecodeError::JumpTargetOutOfRange {
                    target,
                    instrs: code.instrs.len(),
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
fn own_registers(instr: &Instr) -> Vec<u16> {
    match instr {
        Instr::LoadLiteral { dst, .. }
        | Instr::LoadSelf { dst }
        | Instr::LoadBuiltinClass { dst, .. }
        | Instr::MakeClosure { dst, .. } => vec![*dst],
        Instr::Move { dst, src } => vec![*dst, *src],
        // `slot` is a register index too: `run_code` writes it with
        // `write_local`, exactly as it does a `Move`'s `dst`.
        Instr::StoreLocal { src, slot } => vec![*src, *slot],
        Instr::LoadOuterLocal { dst, .. } => vec![*dst],
        Instr::StoreOuterLocal { src, .. } => vec![*src],
        Instr::SendLexical { arg_regs, dst, .. } | Instr::SendSuper { arg_regs, dst, .. } => {
            let mut regs = arg_regs.clone();
            regs.push(*dst);
            regs
        }
        Instr::SendPublic {
            receiver_reg,
            arg_regs,
            dst,
            ..
        } => {
            let mut regs = arg_regs.clone();
            regs.push(*receiver_reg);
            regs.push(*dst);
            regs
        }
        Instr::MakeArray { dst, elem_regs } => {
            let mut regs = elem_regs.clone();
            regs.push(*dst);
            regs
        }
        Instr::InitSlot { src, .. } => vec![*src],
        Instr::Return { reg }
        | Instr::ImplicitReturn { reg }
        | Instr::NonLocalReturn { reg }
        | Instr::JumpIfNotBoolean { reg, .. }
        | Instr::BranchIfFalse { reg, .. }
        | Instr::BranchIfTrue { reg, .. } => vec![*reg],
        Instr::Jump { .. } => vec![],
    }
}

/// Exhaustive, like `own_registers` and for the same reason: a new
/// jump-carrying `Instr` variant must fail to compile here rather than slip
/// through a wildcard and reach `run_code`'s `pc` unchecked.
fn jump_target(instr: &Instr) -> Option<u32> {
    match instr {
        Instr::Jump { target }
        | Instr::JumpIfNotBoolean { target, .. }
        | Instr::BranchIfFalse { target, .. }
        | Instr::BranchIfTrue { target, .. } => Some(*target),
        Instr::LoadLiteral { .. }
        | Instr::LoadSelf { .. }
        | Instr::LoadBuiltinClass { .. }
        | Instr::Move { .. }
        | Instr::StoreLocal { .. }
        | Instr::LoadOuterLocal { .. }
        | Instr::StoreOuterLocal { .. }
        | Instr::SendLexical { .. }
        | Instr::SendPublic { .. }
        | Instr::SendSuper { .. }
        | Instr::MakeClosure { .. }
        | Instr::MakeArray { .. }
        | Instr::InitSlot { .. }
        | Instr::Return { .. }
        | Instr::ImplicitReturn { .. }
        | Instr::NonLocalReturn { .. } => None,
    }
}

fn verify_outer_locals(
    table: &DecodedCodeTable,
    parents: &[Option<usize>],
    index: usize,
    code: &CompiledCode,
) -> Result<(), DecodeError> {
    for instr in &code.instrs {
        let (depth, local) = match instr {
            Instr::LoadOuterLocal { depth, index, .. }
            | Instr::StoreOuterLocal { depth, index, .. } => (*depth, *index),
            _ => continue,
        };
        let enclosing = &table.codes[enclosing_body(table, parents, index, depth)];
        if local >= enclosing.num_registers {
            return Err(DecodeError::OuterLocalOutOfRange {
                depth,
                index: local,
                num_registers: enclosing.num_registers,
            });
        }
    }
    Ok(())
}

/// `eval::frame_at_depth`'s static counterpart: the body of the activation
/// `depth` hops out from an activation of `table.codes[at]`.
///
/// The two agree because `verify_graph` and `vm::run_code` between them
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
    mut at: usize,
    depth: u16,
) -> usize {
    for _ in 0..depth {
        if table.codes[at].kind != CodeKind::Block {
            break;
        }
        match parents[at] {
            Some(parent) => at = parent,
            None => break,
        }
    }
    at
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
        9 => Literal::UnresolvedIdent(r.read_str()?),
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
/// (`interp`), the reloaded root class, the decoded code table, the
/// reloaded module's bindings, and the interpreter's `platform` singleton.
pub struct DecodeContext<'a> {
    pub interp: &'a Interpreter,
    pub root_class: &'a ClassRef,
    pub codes: &'a [Rc<CompiledCode>],
    pub bindings: &'a Rc<HashMap<Span, Binding>>,
    pub platform: Value,
}

pub struct DecodedGraph {
    pub roots: Vec<Value>,
    /// The largest identity hash of any decoded object -- `load` advances
    /// the interpreter's allocation counter past it.
    pub max_identity_hash: u32,
    /// The two decoded tables themselves, in image order. The roots reach
    /// most of them, but `verify_graph` must see *every* record -- a frame
    /// no root reaches can still be some closure's captured frame.
    pub objects: Vec<ObjRef>,
    pub frames: Vec<FrameRef>,
}

/// The two placeholder-allocated tables, indexed by the ids in the stream.
struct Tables<'a> {
    objects: &'a [ObjRef],
    frames: &'a [FrameRef],
}

pub fn decode_class_tag(r: &mut Reader, ctx: &DecodeContext) -> Result<ClassRef, DecodeError> {
    match r.read_u8()? {
        0 => Ok(ctx.interp.builtin_class(read_builtin_class(r)?)),
        1 => Ok(ctx.root_class.clone()),
        tag => Err(DecodeError::InvalidTag { what: "class", tag }),
    }
}

fn decode_value(
    r: &mut Reader,
    ctx: &DecodeContext,
    tables: &Tables,
) -> Result<Value, DecodeError> {
    match r.read_u8()? {
        0 => Ok(Value::Nil),
        1 => Ok(Value::Bool(false)),
        2 => Ok(Value::Bool(true)),
        3 => Ok(Value::SmallInt(r.read_i64()?)),
        4 => Ok(Value::Float(r.read_f64()?)),
        5 => {
            let id = r.read_u32()?;
            tables
                .objects
                .get(id as usize)
                .cloned()
                .map(Value::Obj)
                .ok_or(DecodeError::BadObjectId(id))
        }
        6 => Ok(Value::Obj(decode_class_tag(r, ctx)?)),
        7 => Ok(ctx.platform.clone()),
        tag => Err(DecodeError::InvalidTag { what: "value", tag }),
    }
}

fn decode_values(
    r: &mut Reader,
    ctx: &DecodeContext,
    tables: &Tables,
) -> Result<Vec<Value>, DecodeError> {
    let n = r.read_u32()? as usize;
    let mut values = reserve_for(n, r);
    for _ in 0..n {
        values.push(decode_value(r, ctx, tables)?);
    }
    Ok(values)
}

fn decode_frame_id(r: &mut Reader, tables: &Tables) -> Result<FrameRef, DecodeError> {
    let id = r.read_u32()?;
    tables
        .frames
        .get(id as usize)
        .cloned()
        .ok_or(DecodeError::BadFrameId(id))
}

fn decode_frame_ref(r: &mut Reader, tables: &Tables) -> Result<Option<FrameRef>, DecodeError> {
    if r.read_bool()? {
        Ok(Some(decode_frame_id(r, tables)?))
    } else {
        Ok(None)
    }
}

fn decode_closure_record(
    r: &mut Reader,
    ctx: &DecodeContext,
    tables: &Tables,
) -> Result<ClosureData, DecodeError> {
    let n = r.read_u16()? as usize;
    let mut params = reserve_for(n, r);
    for _ in 0..n {
        params.push(decode_symbol(r)?);
    }
    let code_id = r.read_u32()?;
    let code = ctx
        .codes
        .get(code_id as usize)
        .cloned()
        .ok_or(DecodeError::BadCodeId(code_id))?;
    // The one place in the format that can see both halves of a closure's
    // arity. Checked before the frame ids are read, so a mismatch is
    // reported as itself rather than as whatever the next field trips on.
    if params.len() != code.num_args as usize {
        return Err(DecodeError::ClosureArityMismatch {
            params: params.len(),
            num_args: code.num_args,
        });
    }
    let captured_frame = decode_frame_id(r, tables)?;
    let home = decode_frame_id(r, tables)?;
    Ok(ClosureData {
        params,
        // A decoded closure always carries compiled code: the encoder
        // refuses a `ClosureBody::Ast` outright, so the format has no
        // representation for one.
        body: ClosureBody::Compiled(code),
        captured_frame,
        home,
        // Never serialized -- the reloaded module's map is authoritative.
        bindings: ctx.bindings.clone(),
    })
}

/// Re-resolves a `FrameKind::Method` against the *reloaded* class's method
/// map: the image names the method by selector, never by identity.
///
/// Always `ctx.root_class`, even though the frame record carries a
/// `current_class` of its own that was decoded a few bytes earlier: the
/// same single-user-class invariant `encode_class_tag` documents means the
/// only class that can define a method here *is* the root class. When
/// nested classes load, this must resolve against the frame's own
/// `current_class` instead -- and until then, doing so would be a
/// distinction without a difference that reads like a guarantee.
fn method_decl(ctx: &DecodeContext, selector: String) -> Result<Rc<ast::MethodDecl>, DecodeError> {
    let found = {
        let borrowed = ctx.root_class.borrow();
        match &borrowed.data {
            ObjectData::Class(data) => data
                .application
                .mixin
                .methods
                .get(&Symbol::intern(&selector))
                .cloned(),
            _ => None,
        }
    };
    found.ok_or(DecodeError::UnknownSelector(selector))
}

fn decode_frame(
    r: &mut Reader,
    ctx: &DecodeContext,
    tables: &Tables,
) -> Result<Frame, DecodeError> {
    let receiver = decode_value(r, ctx, tables)?;
    let args = decode_values(r, ctx, tables)?;
    let temps = decode_values(r, ctx, tables)?;
    let pc = r.read_u32()? as usize;
    let sender = decode_frame_ref(r, tables)?;
    let home = decode_frame_ref(r, tables)?;
    let current_class = if r.read_bool()? {
        Some(decode_class_tag(r, ctx)?)
    } else {
        None
    };
    let kind = match r.read_u8()? {
        0 => FrameKind::Method(method_decl(ctx, r.read_str()?)?),
        1 => FrameKind::Closure(Rc::new(decode_closure_record(r, ctx, tables)?)),
        2 => FrameKind::TopLevel,
        3 => FrameKind::Initializer(decode_class_tag(r, ctx)?),
        tag => {
            return Err(DecodeError::InvalidTag {
                what: "frame kind",
                tag,
            })
        }
    };
    let uncontinuable = r.read_bool()?;
    Ok(Frame {
        receiver,
        args,
        temps,
        pc,
        sender,
        home,
        current_class,
        kind,
        uncontinuable,
    })
}

/// A safe, fully initialized stand-in, overwritten in place by
/// `decode_graph_section`'s second pass before anything can observe it.
fn placeholder_frame() -> Frame {
    Frame {
        receiver: Value::Nil,
        args: Vec::new(),
        temps: Vec::new(),
        pc: 0,
        sender: None,
        home: None,
        current_class: None,
        kind: FrameKind::TopLevel,
        uncontinuable: false,
    }
}

/// The smallest object record the wire admits: a `UserRoot` class tag (1),
/// an identity hash (4), and a body tag (1). Every body adds at least four
/// more bytes, so this is a floor, not a typical size.
const MIN_OBJECT_RECORD_BYTES: usize = 6;

/// The smallest frame record the wire admits: a `nil` receiver (1), empty
/// `args` and `temps` counts (4 + 4), a `pc` (4), absent `sender`, `home`
/// and `current_class` (1 + 1 + 1), a `TopLevel` kind tag (1), and
/// `uncontinuable` (1). Every other frame kind is larger.
const MIN_FRAME_RECORD_BYTES: usize = 18;

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
fn bounded_count(count: usize, bytes_per_record: usize, r: &Reader) -> Result<usize, DecodeError> {
    if count > r.remaining() / bytes_per_record {
        return Err(DecodeError::UnexpectedEof);
    }
    Ok(count)
}

/// Two-pass decoding: placeholder-allocate every object and every frame,
/// then fill each in place (so cycles and shared references round-trip
/// with identity preserved), then decode the roots.
///
/// Both passes are flat loops over the two tables, and every reference
/// between records is an id into them, never an inline nested record -- so
/// no part of this decoder recurses on a depth the image controls. The one
/// depth-bounded structure in the format, `Literal::Array`, lives in the
/// code table (`decode_literal`'s `MAX_LITERAL_DEPTH`).
pub fn decode_graph_section(
    r: &mut Reader,
    ctx: &DecodeContext,
) -> Result<DecodedGraph, DecodeError> {
    let object_count = bounded_count(r.read_u32()? as usize, MIN_OBJECT_RECORD_BYTES, r)?;
    let frame_count = bounded_count(r.read_u32()? as usize, MIN_FRAME_RECORD_BYTES, r)?;
    let placeholder_class = ctx.interp.builtin_class(BuiltinClass::Object);
    let objects: Vec<ObjRef> = (0..object_count)
        .map(|_| {
            Rc::new(RefCell::new(Object {
                class: placeholder_class.clone(),
                identity_hash: 0,
                data: ObjectData::Instance(Vec::new()),
            }))
        })
        .collect();
    let frames: Vec<FrameRef> = (0..frame_count)
        .map(|_| Rc::new(RefCell::new(placeholder_frame())))
        .collect();
    let tables = Tables {
        objects: &objects,
        frames: &frames,
    };
    let mut max_identity_hash = 0;
    for obj in &objects {
        let class = decode_class_tag(r, ctx)?;
        let identity_hash = r.read_u32()?;
        // Checked here, the one place identity hashes are read: `load`
        // hands the maximum straight to `advance_identity_hash_past`, which
        // must be able to allocate past it.
        if identity_hash == u32::MAX {
            return Err(DecodeError::IdentityHashExhausted);
        }
        max_identity_hash = max_identity_hash.max(identity_hash);
        let data = match r.read_u8()? {
            0 => ObjectData::Instance(decode_values(r, ctx, &tables)?),
            1 => ObjectData::Str(r.read_str()?),
            2 => ObjectData::Sym(decode_symbol(r)?),
            3 => ObjectData::Array(decode_values(r, ctx, &tables)?),
            4 => ObjectData::Closure(decode_closure_record(r, ctx, &tables)?),
            tag => {
                return Err(DecodeError::InvalidTag {
                    what: "object body",
                    tag,
                })
            }
        };
        *obj.borrow_mut() = Object {
            class,
            identity_hash,
            data,
        };
    }
    for frame in &frames {
        let decoded = decode_frame(r, ctx, &tables)?;
        *frame.borrow_mut() = decoded;
    }
    let roots = decode_values(r, ctx, &tables)?;
    Ok(DecodedGraph {
        roots,
        max_identity_hash,
        objects,
        frames,
    })
}

// ---------------------------------------------------------------------------
// Reconciling the decoded graph with the code that runs in it.
//
// `decode_graph_section` reads frames and closures in isolation: it has no
// selector-to-body mapping, so it can check a closure record's own two
// halves (`ClosureArityMismatch`) and nothing more. Once the code table is
// installed as a `CompiledModule`, every frame's body can be resolved --
// and must be, because `eval::read_local` splits a frame at `args.len()`
// and indexes `temps` unchecked. A crafted frame that is smaller than the
// body addressing it is an out-of-bounds index reached from a
// version-matching image.
// ---------------------------------------------------------------------------

/// The compiled body that runs (or ran) in `frame`, or `None` when the
/// image gives it none: a `TopLevel` frame, or an initializer frame for a
/// class the module has no initializer for. Neither can be entered --
/// nothing re-activates a decoded frame, and no closure may be captured by
/// one (`verify_closure` refuses that) -- so there is nothing to reconcile.
fn frame_body(frame: &Frame, module: &CompiledModule) -> Option<Rc<CompiledCode>> {
    match &frame.kind {
        FrameKind::Method(decl) => module.methods.get(&method_key(decl)).cloned(),
        FrameKind::Initializer(class) => module.initializers.get(&class_key(class)).cloned(),
        FrameKind::Closure(closure) => match &closure.body {
            ClosureBody::Compiled(code) => Some(code.clone()),
            // Unreachable from the wire: the format has no representation
            // for a tree-walker closure (`encode_closure_record` refuses
            // one outright), so this is the exhaustiveness arm.
            ClosureBody::Ast(_) => None,
        },
        FrameKind::TopLevel => None,
    }
}

/// Every decoded frame and every decoded closure against the code table
/// `install_code_table` was handed, closing the two gaps a per-record
/// decoder cannot see:
///
/// 1. A frame's register file must match the body that runs in it -- the
///    same `args` count, and at least as many registers. (At least, not
///    exactly: `run_own_initializer` *grows* the initializer frame's temps
///    to fit each class in the chain, so a legitimate initializer frame can
///    be wider than the body being verified against it.)
/// 2. A closure's `captured_frame` must be an activation of the body its
///    own body is lexically nested in. Nothing else in the format ties the
///    two together, and `verify_code_table`'s outer-local bound -- which
///    walks the *static* `MakeClosure` nesting -- is only a bound on the
///    real `frame_at_depth` walk because of this.
pub fn verify_graph(
    graph: &DecodedGraph,
    table: &DecodedCodeTable,
    module: &CompiledModule,
) -> Result<(), DecodeError> {
    // Recomputed rather than threaded through from `verify_code_table`:
    // one more linear pass over the code table at load time, in exchange
    // for a function that is safe to call on its own.
    let parents = lexical_parents(table)?;
    let index_of = code_index(table);
    for frame in &graph.frames {
        let borrowed = frame.borrow();
        let Some(code) = frame_body(&borrowed, module) else {
            continue;
        };
        if borrowed.args.len() != code.num_args as usize
            || borrowed.args.len() + borrowed.temps.len() < code.num_registers as usize
        {
            return Err(DecodeError::FrameRegisterFile {
                args: borrowed.args.len(),
                locals: borrowed.temps.len(),
                num_args: code.num_args,
                num_registers: code.num_registers,
            });
        }
    }
    for object in &graph.objects {
        if let ObjectData::Closure(closure) = &object.borrow().data {
            verify_closure(closure, table, &parents, &index_of, module)?;
        }
    }
    for frame in &graph.frames {
        if let FrameKind::Closure(closure) = &frame.borrow().kind {
            verify_closure(closure, table, &parents, &index_of, module)?;
        }
    }
    Ok(())
}

fn verify_closure(
    closure: &ClosureData,
    table: &DecodedCodeTable,
    parents: &[Option<usize>],
    index_of: &HashMap<usize, usize>,
    module: &CompiledModule,
) -> Result<(), DecodeError> {
    let ClosureBody::Compiled(code) = &closure.body else {
        // As in `frame_body`: the format cannot express an AST body.
        return Err(DecodeError::WrongCodeKind { expected: "block" });
    };
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
    let running = frame_body(&closure.captured_frame.borrow(), module)
        .ok_or(DecodeError::CapturedFrameMismatch)?;
    if !Rc::ptr_eq(&running, &table.codes[parent]) {
        return Err(DecodeError::CapturedFrameMismatch);
    }
    Ok(())
}
