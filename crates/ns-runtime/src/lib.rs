#![warn(clippy::all)]

//! Name resolution, object model, tree-walking interpreter, and bytecode
//! compiler/VM for the Newspeak3 language (Milestones 1-3), including
//! nested classes, mixin applications, and the source-free image format.
//! See `docs/superpowers/specs/2026-09-01-2252-ns-next-runtime-design.md`
//! and
//! `docs/superpowers/specs/2026-09-06-1659-ns-next-nesting-mixins-source-free-images-design.md`.

pub mod builtins;
pub mod bytecode;
pub mod compiler;
pub mod dispatch;
pub mod image;
pub mod interpreter;
pub mod mixin;
pub mod platform;
pub mod resolver;
pub mod symbol;
pub mod types;
pub mod vm;

pub use interpreter::{Engine, EngineUnavailable, Interpreter, Signal};
pub use mixin::{Mixin, MixinApplication};
pub use resolver::resolve;
pub use symbol::Symbol;
pub use types::*;
