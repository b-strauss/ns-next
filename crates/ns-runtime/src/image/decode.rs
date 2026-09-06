//! Canonical decoding, the inverse of `encode.rs`. A malformed byte stream
//! is a `DecodeError`, never a panic.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_syntax::Span;

use crate::bytecode::{
    method_key, CallSite, CodeKind, CompiledCode, CompiledModule, Instr, Literal,
};
use crate::dispatch::class_application;
use crate::interpreter::instantiate;
use crate::mixin::mixin_key;
use crate::types::{
    Binding, BuiltinClass, ClassRef, ClosureBody, ClosureData, Frame, FrameKind, FrameRef, ObjRef,
    Object, ObjectData, SourceOrigin, Value,
};
use crate::Interpreter;
use crate::Symbol;

use super::{mixins, symbols};

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
    /// A class object's own `class` is another record in the image's object
    /// table. `Interpreter::new_class_object` always gives a class object
    /// the shared bootstrap `Object` placeholder, so no real world contains
    /// one -- and `Object`'s iterative `Drop` cannot hand the `class` field
    /// to its worklist, so a chain of them is the one link left that would
    /// still release recursively. Refusing keeps it one link long.
    ClassOfAClassIsARecord,
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
    /// The mixin table's rows are not `mixin::preorder` of row 0: a row is
    /// named as a child by two rows or by none, a row is its own ancestor,
    /// or the children are named out of the order the walk visits them in.
    /// Row position is the whole correspondence between a row and the
    /// declaration it describes, so a table that does not reproduce that
    /// walk is refused outright rather than paired up wrongly.
    MixinTableNotATree {
        row: u32,
    },
    /// One mixin-table row named the same selector twice, which would leave
    /// its rebuilt method dictionary smaller than its own code list.
    DuplicateSelector(String),
    /// A selector or name reference (a send's selector, a method-dictionary
    /// key, a closure's parameter name, a `Sym` object's text, ...) that
    /// indexes past the end of the image's symbol table. Every such
    /// reference is a `u32` id checked against the table at the point of
    /// use, never trusted to have been validated elsewhere.
    BadSymbolId(u32),
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
    /// The reloaded source declares a superclass clause for a mixin the
    /// image carries no compiled clause for. The nested-class accessor runs
    /// that code on the first access to the declaration, and panics on the
    /// invariant when the module has none -- the same reason
    /// `MissingInitializer` is a refusal rather than a deferred crash.
    MissingSuperclassClause,
    /// The reloaded source's superclass clause carries a factory message
    /// with arguments the image has no compiled body for. `instantiate`
    /// evaluates that body on every instantiation of the class, and panics
    /// on the invariant when the module has none -- the same refusal, for
    /// the same reason, as `MissingSuperclassClause`.
    MissingSuperclassMessage,
    /// The image's code table has a different number of mixin sections than
    /// `mixin::preorder` finds in the reloaded source's mixin tree -- a
    /// nested class added or removed since the snapshot. No mixin identity
    /// survives a save/reload, so position is the only correspondence
    /// `install_code_table` has between an image's sections and the
    /// reloaded tree; a length mismatch means that correspondence cannot be
    /// trusted at all; refusing outright is safer than pairing sections
    /// with the wrong mixins.
    MixinCountMismatch {
        image: usize,
        source: usize,
    },
    /// The image's mixin table and the reloaded source's mixin tree agree
    /// about how many declarations there are but not about which one is at
    /// a given position -- a class renamed, or the declarations reordered,
    /// since the snapshot. Position is the whole correspondence, so pairing
    /// them anyway would install one class's compiled bodies on another's
    /// declarations.
    MixinNameMismatch {
        image: String,
        source: String,
    },
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
            DecodeError::BadSymbolId(id) => write!(f, "symbol id {id} out of range"),
            DecodeError::BadMixinId(id) => write!(f, "mixin id {id} out of range"),
            DecodeError::NotAClassReference => write!(
                f,
                "an image field that must name a class object names an ordinary object instead"
            ),
            DecodeError::ClassOfAClassIsARecord => write!(
                f,
                "a class object in the image is itself an instance of another class object in the \
                 image; every class object's own class is the bootstrap `Object` placeholder"
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
                "the image's mixin table is not a declaration tree: row {row} is reached out of \
                 declaration order, twice, or not at all"
            ),
            DecodeError::DuplicateSelector(s) => write!(
                f,
                "a mixin-table row names method `{s}` more than once"
            ),
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
            DecodeError::MissingSuperclassClause => write!(
                f,
                "the reloaded source declares a superclass clause the image has no compiled code for"
            ),
            DecodeError::MissingSuperclassMessage => write!(
                f,
                "the reloaded source's superclass clause passes factory arguments the image has no compiled code for"
            ),
            DecodeError::MixinCountMismatch { image, source } => write!(
                f,
                "the image carries {image} mixin section(s), but the reloaded source's class tree has {source} -- a nested class was added or removed since the snapshot"
            ),
            DecodeError::MixinNameMismatch { image, source } => write!(
                f,
                "the image's mixin table describes class `{image}` where the reloaded source has `{source}` -- a class was renamed or reordered since the snapshot"
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

    /// Bytes already consumed -- the offset of whatever comes next. Lets a
    /// caller that decoded a known-shape prefix (the header, the source, a
    /// fixed section) locate exactly where the next section starts, e.g. to
    /// corrupt one field of it in a hostile-input test without hardcoding
    /// byte offsets that would drift as the format's earlier fields change.
    pub fn pos(&self) -> usize {
        self.pos
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

/// The decoded code table: every compiled body the image carries, flat and
/// in the order it was written. Which body belongs to which declaration is
/// the mixin table's business (`image::mixins`), which names these by index.
#[derive(Debug, PartialEq)]
pub struct DecodedCodeTable {
    pub codes: Vec<Rc<CompiledCode>>,
}

pub fn decode_code_table(
    r: &mut Reader,
    symbols: &symbols::Table,
) -> Result<DecodedCodeTable, DecodeError> {
    let count = r.read_u32()? as usize;
    let mut codes: Vec<Rc<CompiledCode>> = reserve_for(count, r);
    for _ in 0..count {
        let code = decode_code(r, &codes, symbols)?;
        codes.push(Rc::new(code));
    }
    Ok(DecodedCodeTable { codes })
}

/// `Vec::with_capacity` for a count read from an untrusted stream: every
/// item costs at least one byte, so the bytes still unread are an upper
/// bound on how many can really follow. Without the clamp a corrupt count
/// prefix would reserve gigabytes and abort the process before the read
/// could fail with `UnexpectedEof`.
pub(crate) fn reserve_for<T>(count: usize, r: &Reader) -> Vec<T> {
    Vec::with_capacity(count.min(r.remaining()))
}

fn check_code_id(id: u32, codes: &[Rc<CompiledCode>]) -> Result<(), DecodeError> {
    if (id as usize) < codes.len() {
        Ok(())
    } else {
        Err(DecodeError::BadCodeId(id))
    }
}

/// Binds a decoded image's compiled bodies to the *reloaded* root class:
/// each method to the reloaded mixin's own `Rc<MixinMethod>`, each
/// initializer to the reloaded mixin it belongs to. The image's bytecode is
/// what runs after a load.
///
/// Walks the decoded mixin table against `mixin::preorder(reloaded root)` in
/// lockstep: the two must describe the same declarations in the same order,
/// which is why `mixins::plan` and `compile_module` both walk the same
/// shared `preorder`, and why `mixins::decode` refuses a table whose rows
/// are not that walk. A length mismatch means the reloaded source's mixin
/// tree has a different shape than the one the image was compiled from (a
/// nested class added or removed) -- a decode error, not a panic or a
/// silently truncated zip that would leave some mixin's methods
/// uninstalled. A *name* mismatch at the same position means the shapes
/// happen to agree while the declarations do not, which the row count alone
/// cannot see: refusing is what stops one class's compiled bodies from being
/// installed on another's declarations.
///
/// (Once `load` builds its class objects from the mixin table itself rather
/// than from re-parsed source, this reconciliation has only one side left
/// and goes away with the re-parse.)
pub fn install_code_table(
    table: &DecodedCodeTable,
    mixins: &mixins::Decoded,
    root_class: &ClassRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> Result<CompiledModule, DecodeError> {
    let mut module = CompiledModule {
        methods: HashMap::new(),
        initializers: HashMap::new(),
        superclass_clauses: HashMap::new(),
        superclass_messages: HashMap::new(),
        bindings: bindings.clone(),
    };
    let mixin_order = source_mixin_order(root_class)?;
    if mixin_order.len() != mixins.code.len() {
        return Err(DecodeError::MixinCountMismatch {
            image: mixins.code.len(),
            source: mixin_order.len(),
        });
    }
    for ((mixin, decoded), section) in mixin_order.iter().zip(&mixins.mixins).zip(&mixins.code) {
        if decoded.name != mixin.name {
            return Err(DecodeError::MixinNameMismatch {
                image: decoded.name.as_str().to_string(),
                source: mixin.name.as_str().to_string(),
            });
        }
        for (selector, id) in &section.methods {
            let decl = mixin
                .methods
                .get(selector)
                .ok_or_else(|| DecodeError::UnknownSelector(selector.as_str().to_string()))?;
            let code = code_at(table, *id)?;
            // The frame an installed body runs in is built by the shared
            // orchestration, not by the body: `invoke_method` fills `args`
            // from the send (so, from the selector's arity) and sizes
            // `temps` from the body's `num_args`. A body whose arity
            // disagrees with the reloaded declaration therefore leaves
            // `read_local` splitting a frame it does not fit -- reading
            // `temps` past its end for a body that claims too many
            // arguments. Only this function sees both.
            check_code_arity(
                &code,
                CodeKind::Method,
                format!("method `{selector}`"),
                decl.num_args,
            )?;
            module.methods.insert(method_key(decl), code);
        }
        // The other direction: a method the reloaded source defines but the
        // image has no code for would leave `module.methods` with a hole,
        // and dispatching it panics in the engine hook. That is the
        // ordinary "source changed since the snapshot" path, so it is a
        // decode error here rather than a crash later. Sorted, so the
        // reported selector does not depend on the mixin's method
        // `HashMap` order. Scoped to *this* mixin's own methods only --
        // `module.methods` accumulates every mixin's methods as the loop
        // proceeds, but `method_key` is `Rc<MixinMethod>` identity, so
        // checking `mixin.methods` here can never see a different mixin's
        // entry as if it were this one's.
        let mut missing: Vec<&str> = mixin
            .methods
            .iter()
            .filter(|(_, decl)| !module.methods.contains_key(&method_key(decl)))
            .map(|(selector, _)| selector.as_str())
            .collect();
        missing.sort_unstable();
        if let Some(selector) = missing.first() {
            return Err(DecodeError::MissingSelector((*selector).to_string()));
        }
        match section.initializer {
            Some(id) => {
                let code = code_at(table, id)?;
                // The same reconciliation for the initializer, whose
                // `num_args` is the mixin's factory parameter count
                // (`compile_initializer`) and whose frame's `args` are the
                // factory arguments `instantiate` checked against that same
                // pattern.
                check_code_arity(
                    &code,
                    CodeKind::Initializer,
                    "instance initializer".to_string(),
                    mixin.factory.map_or(0, |factory| factory.num_args),
                )?;
                module.initializers.insert(mixin_key(mixin), code);
            }
            // And the initializer's `MissingSelector`: a class with slots
            // or `initExpr`s runs compiled initializer code on every
            // `X new`, and `run_own_initializer` *panics* when the module
            // has none. The reloaded source decides whether the mixin
            // needs it, and the image decides whether it carries it -- two
            // halves only this function sees, exactly as for the method
            // table above.
            None if instantiate::mixin_needs_initializer_code(mixin) => {
                return Err(DecodeError::MissingInitializer)
            }
            None => {}
        }
        match section.superclass_clause {
            Some(id) => {
                let code = code_at(table, id)?;
                // A clause body takes no arguments: it is evaluated in an
                // activation the *accessor* builds on the enclosing object,
                // not in one carrying the nested class's factory arguments.
                check_code_arity(
                    &code,
                    CodeKind::SuperclassClause,
                    "superclass clause".to_string(),
                    0,
                )?;
                module.superclass_clauses.insert(mixin_key(mixin), code);
            }
            // The same reconciliation as the initializer's: a reloaded
            // mixin that declares a superclass clause has its accessor run
            // that clause's code on the first nested-class access, and
            // `interpreter::nested` panics when the module carries none.
            // Whether the reloaded source declares one and whether the
            // image carries it are two halves only this function sees.
            None if mixin.superclass.is_some() => return Err(DecodeError::MissingSuperclassClause),
            None => {}
        }
        match section.superclass_message {
            Some(id) => {
                let code = code_at(table, id)?;
                // Unlike the clause's receiver, the clause's *message*
                // arguments run in an activation carrying this mixin's own
                // factory arguments (`newspeak-spec.pdf` §6.2), so their
                // body's arity is the factory pattern's -- the same
                // reconciliation the instance initializer above gets, for
                // the same reason.
                check_code_arity(
                    &code,
                    CodeKind::SuperclassMessage,
                    "superclass clause message".to_string(),
                    mixin.factory.map_or(0, |factory| factory.num_args),
                )?;
                module.superclass_messages.insert(mixin_key(mixin), code);
            }
            // A reloaded mixin whose clause message carries arguments
            // evaluates them on every instantiation, and
            // `instantiate::eval_superclass_message` panics when the module
            // carries no body for them. Whether the reloaded source needs
            // one and whether the image has one are two halves only this
            // function sees.
            None if mixin_needs_superclass_message_code(mixin) => {
                return Err(DecodeError::MissingSuperclassMessage)
            }
            None => {}
        }
    }
    Ok(module)
}

/// Whether `mixin`'s superclass clause carries a message with arguments, and
/// therefore needs a compiled body for them. Mirrors
/// `instantiate::mixin_needs_initializer_code`: the one question
/// `install_code_table` asks of the reloaded source, answered off the same
/// field `instantiate::eval_superclass_message` decides by at run time.
fn mixin_needs_superclass_message_code(mixin: &Rc<crate::mixin::Mixin>) -> bool {
    mixin.superclass.is_some_and(|clause| clause.has_args)
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
                CodeKind::SuperclassClause => "superclass clause",
                CodeKind::SuperclassMessage => "superclass clause message",
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

fn decode_code(
    r: &mut Reader,
    codes: &[Rc<CompiledCode>],
    symbols: &symbols::Table,
) -> Result<CompiledCode, DecodeError> {
    let kind = match r.read_u8()? {
        0 => CodeKind::Method,
        1 => CodeKind::Block,
        2 => CodeKind::Initializer,
        3 => CodeKind::SuperclassClause,
        4 => CodeKind::SuperclassMessage,
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
        instrs.push(decode_instr(r, codes, symbols)?);
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

/// The table lookup that replaces a per-occurrence `Symbol::intern`: every
/// selector or name in the code table is a `u32` id into the symbol table
/// decoded once, up front, in `image::load`.
fn decode_symbol(r: &mut Reader, symbols: &symbols::Table) -> Result<Symbol, DecodeError> {
    symbols.get(r.read_u32()?)
}

fn decode_instr(
    r: &mut Reader,
    codes: &[Rc<CompiledCode>],
    symbols: &symbols::Table,
) -> Result<Instr, DecodeError> {
    Ok(match r.read_u8()? {
        0 => {
            let dst = r.read_u16()?;
            Instr::LoadLiteral {
                dst,
                literal: decode_literal(r, 0, symbols)?,
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
            let selector = decode_symbol(r, symbols)?;
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
            let selector = decode_symbol(r, symbols)?;
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
            let selector = decode_symbol(r, symbols)?;
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
                params.push(decode_symbol(r, symbols)?);
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
                own_index: r.read_u32()?,
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

fn decode_literal(
    r: &mut Reader,
    depth: usize,
    symbols: &symbols::Table,
) -> Result<Literal, DecodeError> {
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
        5 => Literal::Sym(decode_symbol(r, symbols)?),
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
                elements.push(decode_literal(r, depth + 1, symbols)?);
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
/// reloaded module's bindings, the interpreter's `platform` singleton, and
/// the reloaded declaration tree a class object's mixin id names.
///
/// `interp` is `&mut` because decoding a class object *allocates into* the
/// interpreter: rebuilding its `MixinApplication` goes through
/// `Interpreter::apply_mixin`, and that shared cache is the whole mechanism
/// by which two enclosing instances get back the one application they had
/// before the snapshot.
pub struct DecodeContext<'a> {
    pub interp: &'a mut Interpreter,
    pub root_class: &'a ClassRef,
    pub codes: &'a [Rc<CompiledCode>],
    pub bindings: &'a Rc<HashMap<Span, Binding>>,
    pub platform: Value,
    /// `mixin::preorder` of the *reloaded* root class's mixin, indexed by
    /// mixin-table row id -- `source_mixin_order`. The reloaded tree, not
    /// the one `mixins::decode` rebuilt: `install_code_table` binds the
    /// image's compiled bodies to *these* mixins, so a class object built
    /// around any other copy would dispatch into an empty module.
    pub mixins: &'a [Rc<crate::mixin::Mixin>],
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
pub struct Tables<'a> {
    pub objects: &'a [ObjRef],
    pub frames: &'a [FrameRef],
}

/// The inverse of `encode::encode_class_tag`. Tag 2's object may still be a
/// placeholder when this runs -- a class object can be named before its own
/// record is read -- so this resolves identity only; that the record really
/// was a class record is `check_class_references`'s job, once every record
/// has been read.
pub fn decode_class_tag(
    r: &mut Reader,
    ctx: &DecodeContext,
    tables: &Tables,
) -> Result<ClassRef, DecodeError> {
    match r.read_u8()? {
        0 => Ok(ctx.interp.builtin_class(read_builtin_class(r)?)),
        1 => Ok(ctx.root_class.clone()),
        2 => {
            let id = r.read_u32()?;
            tables
                .objects
                .get(id as usize)
                .cloned()
                .ok_or(DecodeError::BadObjectId(id))
        }
        tag => Err(DecodeError::InvalidTag { what: "class", tag }),
    }
}

fn decode_optional_class(
    r: &mut Reader,
    ctx: &DecodeContext,
    tables: &Tables,
) -> Result<Option<ClassRef>, DecodeError> {
    if r.read_bool()? {
        Ok(Some(decode_class_tag(r, ctx, tables)?))
    } else {
        Ok(None)
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
        6 => Ok(Value::Obj(decode_class_tag(r, ctx, tables)?)),
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
    symbols: &symbols::Table,
) -> Result<ClosureData, DecodeError> {
    let n = r.read_u16()? as usize;
    let mut params = reserve_for(n, r);
    for _ in 0..n {
        params.push(decode_symbol(r, symbols)?);
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
/// Against the frame's own `current_class`, which `dispatch::walk` sets to
/// the level whose mixin declares the running method -- so that level's
/// method dictionary is exactly where the selector is found, and the
/// `Rc<MixinMethod>` it yields is the key `install_code_table` bound this
/// image's compiled body under. Now that a nested class object is an
/// ordinary record, `current_class` is regularly a class other than the
/// root, and resolving everything against the root would either miss the
/// selector or bind the frame to a same-named method of a different class.
/// `None` -- a `TopLevel` activation -- has no class to ask, so the root
/// class stands in, as it did for every frame before nesting.
fn method_decl(
    ctx: &DecodeContext,
    current_class: Option<&ClassRef>,
    selector: Symbol,
) -> Result<Rc<crate::mixin::MixinMethod>, DecodeError> {
    let class = current_class.unwrap_or(ctx.root_class);
    let found = class_application(class).and_then(|a| a.mixin.methods.get(&selector).cloned());
    found.ok_or(DecodeError::UnknownSelector(selector.as_str().to_string()))
}

fn decode_frame(
    r: &mut Reader,
    ctx: &DecodeContext,
    tables: &Tables,
    symbols: &symbols::Table,
) -> Result<Frame, DecodeError> {
    let receiver = decode_value(r, ctx, tables)?;
    let args = decode_values(r, ctx, tables)?;
    let temps = decode_values(r, ctx, tables)?;
    let pc = r.read_u32()? as usize;
    let sender = decode_frame_ref(r, tables)?;
    let home = decode_frame_ref(r, tables)?;
    let current_class = decode_optional_class(r, ctx, tables)?;
    let kind = match r.read_u8()? {
        0 => FrameKind::Method(method_decl(
            ctx,
            current_class.as_ref(),
            decode_symbol(r, symbols)?,
        )?),
        1 => FrameKind::Closure(Rc::new(decode_closure_record(r, ctx, tables, symbols)?)),
        2 => FrameKind::TopLevel,
        3 => FrameKind::Initializer(decode_class_tag(r, ctx, tables)?),
        4 => FrameKind::SuperclassClause(decode_class_tag(r, ctx, tables)?),
        5 => FrameKind::SuperclassMessage(decode_class_tag(r, ctx, tables)?),
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
        // Nothing re-activates a decoded frame (this module's doc comment,
        // below), so no compiled `InitSlot` ever reads this on a frame that
        // came off the wire.
        slot_base: 0,
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
        slot_base: 0,
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

/// The `Rc<Mixin>`s a class object's mixin id names: `mixin::preorder` of
/// the *reloaded* root class's declaration tree, in row-id order.
///
/// The image's mixin table is checked to be that same walk twice over
/// before this is indexed -- `mixins::decode` proves its rows are
/// `preorder` of row 0, and `install_code_table` proves the reloaded tree
/// has the same length and the same name at every position -- which is what
/// makes a row id mean the same declaration on both sides.
pub fn source_mixin_order(
    root_class: &ClassRef,
) -> Result<Vec<Rc<crate::mixin::Mixin>>, DecodeError> {
    let root_mixin = {
        let borrowed = root_class.borrow();
        let ObjectData::Class(data) = &borrowed.data else {
            return Err(DecodeError::NotAClass);
        };
        data.application.mixin.clone()
    };
    let mut order = Vec::new();
    crate::mixin::preorder(&root_mixin, &mut order);
    Ok(order)
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
/// An application is derived, never stored: `Interpreter::apply_mixin`
/// computes the slot layout and the lookup chain from a mixin and its
/// superclass's application, and caches the result under exactly that pair.
/// So rebuilding restores identity as well as content -- two class objects
/// that shared one application before the snapshot ask for the same pair
/// here and get the same `Rc` back, which is what keeps a call site
/// monomorphic across a save/load.
///
/// The order is the catch: a class object's record may name a superclass
/// whose own record appears later in the table. Each class has at most one
/// superclass, so the dependency structure is a forest; this walks each
/// chain to its already-applied end (a builtin, the root class, or a class
/// resolved by an earlier walk) over an explicit stack, then applies back
/// down it. A chain that closes on itself is `SuperclassCycle` rather than
/// a loop that never ends.
fn rebuild_applications(
    ctx: &mut DecodeContext,
    objects: &[ObjRef],
    pending: Vec<PendingClass>,
) -> Result<(), DecodeError> {
    // Class-object address -> position in `pending`, for the "is my
    // superclass still waiting for its own application?" question each step
    // asks. Keyed by address because a superclass arrives as an `Rc`, not
    // as the id it was written with.
    let mut position: HashMap<usize, usize> = HashMap::with_capacity(pending.len());
    for (i, p) in pending.iter().enumerate() {
        position.insert(Rc::as_ptr(&objects[p.object]) as usize, i);
    }
    let mut done = vec![false; pending.len()];
    let mut on_chain = vec![false; pending.len()];
    let mut chain: Vec<usize> = Vec::new();
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
            let superclass = class_superclass(&objects[pending[current].object]);
            let Some(superclass) = superclass else { break };
            let Some(&next) = position.get(&(Rc::as_ptr(&superclass) as usize)) else {
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
        for &i in chain.iter().rev() {
            let object = &objects[pending[i].object];
            let super_application = match class_superclass(object) {
                Some(superclass) => {
                    Some(class_application(&superclass).ok_or(DecodeError::NotAClassReference)?)
                }
                None => None,
            };
            let application = ctx
                .interp
                .apply_mixin(&pending[i].mixin, super_application.as_ref());
            match &mut object.borrow_mut().data {
                ObjectData::Class(data) => data.application = application,
                _ => unreachable!("this object was filled in as a class record"),
            }
            done[i] = true;
            on_chain[i] = false;
        }
        chain.clear();
    }
    Ok(())
}

fn class_superclass(class: &ObjRef) -> Option<ClassRef> {
    match &class.borrow().data {
        ObjectData::Class(data) => data.superclass.clone(),
        _ => None,
    }
}

fn is_class_object(obj: &ObjRef) -> bool {
    matches!(obj.borrow().data, ObjectData::Class(_))
}

/// Whether `class` is one of the two class objects a graph never creates:
/// a builtin, or the module's root class. The mirror of
/// `encode::is_class_tag`.
fn is_prebuilt_class(class: &ClassRef, root_class: &ClassRef) -> bool {
    match &class.borrow().data {
        ObjectData::Class(data) => data.builtin.is_some() || Rc::ptr_eq(class, root_class),
        _ => false,
    }
}

/// Every field the format types as a class reference really names a class
/// object, and a class object's own `class` is one the graph did not
/// create.
///
/// A tag-2 class tag names an object-table id, and nothing in the byte
/// stream forces that record to have been a class record -- which is only
/// knowable once every record has been read, so it is checked here rather
/// than where each tag is decoded. These five positions are the complete
/// set: a class tag appears nowhere else in the graph section, and a plain
/// `Value::Obj` pointing at a class object is legitimate (that is what a
/// cached nested class in a slot *is*).
///
/// The extra rule for a class object's *own* `class` is the runtime's
/// invariant, not a limit invented here: `Interpreter::new_class_object` is
/// the only thing that mints a class object and always gives it the shared
/// bootstrap `Object` placeholder -- §3.2's reified metaclass model stays
/// deliberately unbuilt -- so this refuses nothing a real world contains.
/// It is load-bearing because `class` is the one reference `Object`'s
/// iterative `Drop` cannot hand to its worklist (it is not an `Option`, and
/// the field's glue runs after `drop` returns): bounding the chain at one
/// link is what keeps a crafted image from cascading through it.
fn check_class_references(
    objects: &[ObjRef],
    frames: &[FrameRef],
    root_class: &ClassRef,
) -> Result<(), DecodeError> {
    for object in objects {
        let borrowed = object.borrow();
        if !is_class_object(&borrowed.class) {
            return Err(DecodeError::NotAClassReference);
        }
        if let ObjectData::Class(data) = &borrowed.data {
            if !is_prebuilt_class(&borrowed.class, root_class) {
                return Err(DecodeError::ClassOfAClassIsARecord);
            }
            for class in data.superclass.iter().chain(data.declaring_class.iter()) {
                if !is_class_object(class) {
                    return Err(DecodeError::NotAClassReference);
                }
            }
        }
    }
    for frame in frames {
        let borrowed = frame.borrow();
        let kind_class = match &borrowed.kind {
            FrameKind::Initializer(c)
            | FrameKind::SuperclassClause(c)
            | FrameKind::SuperclassMessage(c) => Some(c),
            FrameKind::Method(_) | FrameKind::Closure(_) | FrameKind::TopLevel => None,
        };
        for class in borrowed.current_class.iter().chain(kind_class) {
            if !is_class_object(class) {
                return Err(DecodeError::NotAClassReference);
            }
        }
    }
    Ok(())
}

/// Two-pass decoding: placeholder-allocate every object and every frame,
/// then fill each in place (so cycles and shared references round-trip
/// with identity preserved), then decode the roots.
///
/// Class objects need one more step between the two: their records are
/// filled in like any other, but the `MixinApplication` a class object
/// carries is derived rather than stored, and deriving it needs the
/// superclass's -- which may itself be a record that appears later.
/// `rebuild_applications` closes that gap once every record has been read.
/// It runs before the frames, whose `FrameKind::Method` resolves a selector
/// against `current_class`'s method dictionary.
///
/// Every pass is a flat loop over the two tables, and every reference
/// between records is an id into them, never an inline nested record -- so
/// no part of this decoder recurses on a depth the image controls. The one
/// depth-bounded structure in the format, `Literal::Array`, lives in the
/// code table (`decode_literal`'s `MAX_LITERAL_DEPTH`).
pub fn decode_graph_section(
    r: &mut Reader,
    ctx: &mut DecodeContext,
    symbols: &symbols::Table,
) -> Result<DecodedGraph, DecodeError> {
    let object_count = bounded_count(r.read_u32()? as usize, MIN_OBJECT_RECORD_BYTES, r)?;
    let frame_count = bounded_count(r.read_u32()? as usize, MIN_FRAME_RECORD_BYTES, r)?;
    let placeholder_class = ctx.interp.builtin_class(BuiltinClass::Object);
    // A real application, standing in until `rebuild_applications` derives
    // each class object's own -- the same technique as the placeholder
    // object and `placeholder_frame`, and for the same reason: nothing may
    // observe a half-built record, and a `None` here would need an
    // `Option` on `ClassData` that no live class object could ever be.
    let placeholder_application =
        class_application(&placeholder_class).ok_or(DecodeError::NotAClass)?;
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
    let mut pending_classes: Vec<PendingClass> = Vec::new();
    for (index, obj) in objects.iter().enumerate() {
        let class = decode_class_tag(r, ctx, &tables)?;
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
            2 => ObjectData::Sym(decode_symbol(r, symbols)?),
            3 => ObjectData::Array(decode_values(r, ctx, &tables)?),
            4 => ObjectData::Closure(decode_closure_record(r, ctx, &tables, symbols)?),
            5 => {
                let (data, mixin) = decode_class_record(r, ctx, &tables, &placeholder_application)?;
                pending_classes.push(PendingClass {
                    object: index,
                    mixin,
                });
                ObjectData::Class(data)
            }
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
    rebuild_applications(ctx, &objects, pending_classes)?;
    for frame in &frames {
        let decoded = decode_frame(r, ctx, &tables, symbols)?;
        *frame.borrow_mut() = decoded;
    }
    check_class_references(&objects, &frames, ctx.root_class)?;
    let roots = decode_values(r, ctx, &tables)?;
    Ok(DecodedGraph {
        roots,
        max_identity_hash,
        objects,
        frames,
    })
}

/// The inverse of `encode::encode_class_record`: the mixin-table row this
/// class applies, then its superclass, its enclosing object and its
/// declaring class.
///
/// The mixin id is range-checked here, against the reloaded declaration
/// tree, before it is used to index anything. The three references resolve
/// to identities that already exist (every record was placeholder-allocated
/// before this pass started), so a forward reference and a cycle are both
/// ordinary; what cannot be settled yet is whether each really is a class
/// record, which `check_class_references` establishes afterwards.
fn decode_class_record(
    r: &mut Reader,
    ctx: &DecodeContext,
    tables: &Tables,
    placeholder_application: &Rc<crate::mixin::MixinApplication>,
) -> Result<(crate::types::ClassData, Rc<crate::mixin::Mixin>), DecodeError> {
    let id = r.read_u32()?;
    let mixin = ctx
        .mixins
        .get(id as usize)
        .cloned()
        .ok_or(DecodeError::BadMixinId(id))?;
    let superclass = decode_optional_class(r, ctx, tables)?;
    let enclosing = if r.read_bool()? {
        match decode_value(r, ctx, tables)? {
            Value::Obj(obj) => Some(obj),
            _ => return Err(DecodeError::EnclosingNotAnObject),
        }
    } else {
        None
    };
    let declaring_class = decode_optional_class(r, ctx, tables)?;
    Ok((
        crate::types::ClassData {
            application: placeholder_application.clone(),
            enclosing,
            declaring_class,
            superclass,
            // A record only ever describes a user class: a builtin is a
            // class *tag*, resolved through the bootstrap, and never an
            // object-table entry.
            builtin: None,
            // Rebuilt by `Interpreter::metaclass_of` on the first
            // class-side send, exactly as it is for a class object the
            // accessor just created.
            //
            // Not the *same* metaclass, though, in one case: a metaclass
            // that was itself in the graph -- as a class-side activation's
            // `current_class` -- decodes as a class record of its own and
            // is never linked back into the class it belongs to, so
            // `metaclass_of` mints a second one beside it. Nothing observes
            // the difference today: `metaclass_of` is reached only from
            // `Interpreter::send`'s dispatch, so no Newspeak value names a
            // metaclass and no two of them can be compared. Recording it
            // rather than claiming stability, because the claim would stop
            // being true the moment §3.2's reified model is built.
            metaclass: None,
        },
        mixin,
    ))
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
/// image gives it none: a `TopLevel` frame, or a class-carrying frame whose
/// kind names a table the module holds no body for that class's mixin in.
/// Neither can be entered -- nothing re-activates a decoded frame, and no
/// closure may be captured by one (`verify_closure` refuses that) -- so
/// there is nothing to reconcile.
fn frame_body(frame: &Frame, module: &CompiledModule) -> Option<Rc<CompiledCode>> {
    /// The mixin whose code table a class-carrying frame kind names. `None`
    /// only if the decoded value is not a class object -- `decode_class_tag`
    /// never produces one, but returning `None` keeps a future change to it
    /// from turning an untrusted image into a panic here.
    fn frame_mixin(class: &crate::types::ClassRef) -> Option<Rc<crate::mixin::Mixin>> {
        class_application(class).map(|a| a.mixin.clone())
    }
    match &frame.kind {
        FrameKind::Method(decl) => module.methods.get(&method_key(decl)).cloned(),
        FrameKind::Initializer(class) => {
            frame_mixin(class).and_then(|m| module.initializers.get(&mixin_key(&m)).cloned())
        }
        // Each clause activation resolves against the table its own body
        // lives in. They share `Initializer`'s frame shape -- a class's code
        // running on an instance -- but not its body, so resolving them here
        // as initializers would reconcile a frame against a body that never
        // ran in it: a legitimate image refused, and a crafted one checked
        // against the wrong register file.
        FrameKind::SuperclassClause(class) => {
            frame_mixin(class).and_then(|m| module.superclass_clauses.get(&mixin_key(&m)).cloned())
        }
        FrameKind::SuperclassMessage(class) => {
            frame_mixin(class).and_then(|m| module.superclass_messages.get(&mixin_key(&m)).cloned())
        }
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
///    exactly: a frame's register file is grown, never shrunk -- the
///    liveness spec's reified activations may be wider than the body
///    currently running in them -- so a wider frame is legitimate and only
///    a narrower one is a crafted image.)
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
