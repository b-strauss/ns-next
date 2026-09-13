#![warn(clippy::all)]

//! Name resolution, object model, bytecode compiler and VM for the
//! Newspeak3 language, including nested classes, mixin
//! applications, and the source-free image format.
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

pub use compiler::Lowering;
pub use heap::{ActivationHeader, ActivationRef, ClassRef, FrameRef, Heap, ObjRef, Root};
pub use interpreter::{HostError, HostResult, HostSignal, Interpreter, Signal};
pub use mixin::{Mixin, MixinApplication};
pub use resolver::resolve;
pub use symbol::Symbol;
pub use types::*;
