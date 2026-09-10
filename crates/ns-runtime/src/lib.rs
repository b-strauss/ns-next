#![warn(clippy::all)]

//! Name resolution, object model, tree-walking interpreter, and bytecode
//! compiler/VM for the Newspeak3 language (Milestones 1-3), including
//! nested classes, mixin applications, and the source-free image format.
//! See `docs/design/` -- start at its `README.md`.

pub mod builtins;
pub mod bytecode;
pub mod compiler;
pub mod dispatch;
pub mod heap;
pub mod image;
pub mod interpreter;
pub mod mixin;
pub mod platform;
pub mod resolver;
pub mod symbol;
pub mod types;
pub mod vm;

pub use heap::{ActivationHeader, ActivationRef, ClassRef, FrameRef, Heap, ObjRef};
pub use interpreter::{Engine, EngineUnavailable, Interpreter, Signal};
pub use mixin::{Mixin, MixinApplication};
pub use resolver::resolve;
pub use symbol::Symbol;
pub use types::*;
