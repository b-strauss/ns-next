//! The values a caller hands the heap to build an object or fill a frame,
//! and the shape it reads one back as. They hold values and handles, never
//! the words or payload types an object is stored as (`object.rs`), so that
//! a change to the representation behind the seam leaves every caller as it
//! is (`docs/design/memory.md`, "The seam").

use std::rc::Rc;

use crate::bytecode::CompiledCode;
use crate::heap::{ClassReference, FrameReference, ObjectReference, Value};
use crate::interpreter::FeedbackBase;
use crate::mixin::MixinApplication;
use crate::types::{BuiltinClass, FrameKind};

/// Which kind of payload an object carries (`Heap::object_shape`): what
/// the encoder and the graph walk branch on before they read the payload
/// through the accessor for that kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectShape {
    Instance,
    String,
    Symbol,
    Array,
    Closure,
    Class,
    Activation,
    LargeInteger,
    Float,
    ByteArray,
    /// A `WeakArray`: its slots hold their objects weakly
    /// (`Heap::weak_array_elements`).
    WeakArray,
    /// An `Ephemeron`: its key, value and finalizer are fields of its own
    /// format (`Heap::ephemeron_fields`), the value and finalizer held only
    /// through a live key.
    Ephemeron,
    /// A reified mixin, the canonical object of the kernel-private `Mixin`
    /// standing for one mixin (`Heap::mixin_object_id`): an instance of
    /// one field, written as the mixin it stands for, not as that field.
    Mixin,
}

/// A class object to build (`Heap::allocate_class`,
/// `Heap::allocate_self_referential_class`): its application and the three
/// references that make it this application of it rather than another, and
/// the builtin it is, if any. Built by the bootstrap, by
/// `Mutator::new_class_object` and `new_applied_level`, by the metaclass
/// a class object gets, and by the image decoder. Whatever a class object
/// is given after it exists -- its metaclass link, the `<:` clause that
/// added it, its value-class verdict -- goes through the `class_set_*`
/// setters instead.
pub struct NewClass<'gc> {
    pub application: Rc<MixinApplication>,
    pub enclosing: Option<ObjectReference<'gc>>,
    pub declaring_class: Option<ClassReference<'gc>>,
    pub superclass: Option<ClassReference<'gc>>,
    pub builtin: Option<BuiltinClass>,
}

/// Where a body-less declaration's own code walks out from
/// (`Heap::class_declaration_scope`): the enclosing object the declaration
/// was evaluated in and the class object of the level whose declaration
/// lexically contains it -- what a body version of the declaration would
/// have as its own -- both `None` for a module's root.
#[derive(Clone, Copy)]
pub struct DeclarationScope<'gc> {
    pub enclosing: Option<ObjectReference<'gc>>,
    pub declaring_class: Option<ClassReference<'gc>>,
}

/// A block closure to build (`Heap::allocate_closure`): the body it runs,
/// shared with the `MakeClosure` that makes it, the frame it was created
/// in, the values it was handed, and the start of its family's range in the
/// cache table -- `FeedbackBase::NOT_ASSIGNED` for one an image decoded.
/// Built by the run loop's `MakeClosure` and by the image decoder.
pub struct NewClosure<'gc> {
    pub code: Rc<CompiledCode>,
    pub captured_frame: FrameReference<'gc>,
    pub copied: Vec<Value<'gc>>,
    pub feedback_base: FeedbackBase,
}

/// The format a placeholder filled in place takes (`Heap::shape_placeholder`):
/// an instance's -- one still under construction among them, a value whose
/// construction was cut short before the image was written -- an `Array`'s,
/// a `WeakArray`'s or an `Ephemeron`'s. The
/// image decoder allocates every record of its object table first, at the
/// size its first pass read, so that a reference may point forward, and
/// fills each in place afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceholderFormat {
    Instance,
    InstanceUnderConstruction,
    Array,
    WeakArray,
    Ephemeron,
}

/// A reified frame's contents as values (`Heap::frame_record`,
/// `Heap::fill_frame_fields`): what the image encoder writes for a frame and the
/// decoder reads back. Only a dead frame has one: a married frame's
/// registers and program counter live on the activation stack.
pub struct FrameRecord<'gc> {
    pub receiver: Value<'gc>,
    pub arguments: Vec<Value<'gc>>,
    /// The registers above the header, in order: register `argument_count
    /// + HEADER_REGISTERS + index` is `temporaries[index]`.
    pub temporaries: Vec<Value<'gc>>,
    pub program_counter: usize,
    pub sender: Option<FrameReference<'gc>>,
    pub home: Option<FrameReference<'gc>>,
    pub current_class: Option<ClassReference<'gc>>,
    pub kind: FrameKind<'gc>,
    pub uncontinuable: bool,
}
