#![warn(clippy::all)]

//! Name resolution, object model, tree-walking interpreter, and bytecode
//! compiler/VM for the Newspeak3 language (Milestones 1-2). See
//! `docs/superpowers/specs/2026-09-01-2252-ns-next-runtime-design.md`.

pub mod builtins;
pub mod bytecode;
pub mod dispatch;
pub mod interpreter;
pub mod platform;
pub mod resolver;
pub mod symbol;
pub mod types;

pub use interpreter::{Interpreter, Signal};
pub use resolver::resolve;
pub use symbol::Symbol;
pub use types::*;
