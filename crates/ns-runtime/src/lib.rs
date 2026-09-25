#![warn(clippy::all)]
// The brand needs no unsafe code: `World::enter` (`heap/world.rs`) hands the
// step state to a closure the compiler checks for every `'gc`.
#![forbid(unsafe_code)]

//! Name resolution, object model, bytecode compiler and VM for the
//! Newspeak3 language, including nested classes, mixin
//! applications, and the source-free image format.
//! See `docs/design/` -- start at its `README.md`.

pub mod builtins;
pub mod bytecode;
pub mod compiler;
pub mod dispatch;
pub mod hash;
pub mod heap;
pub mod image;
pub mod interpreter;
pub mod mixin;
pub mod platform;
pub mod resolver;
pub mod symbol;
pub(crate) mod trace;
pub mod types;
pub mod vm;

pub use compiler::Lowering;
pub use heap::{
    ActivationHeader, ActivationReference, ClassReference, CollectorStatistics, FrameReference,
    Heap, HeapStatistics, ObjectReference, Root,
};
pub use interpreter::{
    CacheStatistics, CachedTarget, CachedTargetKind, CallSiteState, HostError, HostResult,
    HostSignal, Interpreter, MethodSide, Mutator,
};
pub use mixin::{DispatchId, DispatchIds, Mixin, MixinApplication};
pub use resolver::{resolve, resolve_for_editor};
pub use symbol::Symbol;
pub use types::*;
