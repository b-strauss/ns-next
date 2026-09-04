#![warn(clippy::all)]

//! Name resolution, object model, and tree-walking interpreter for the
//! Newspeak3 language (Milestone 1). See
//! `docs/superpowers/specs/2026-09-01-ns-next-runtime-design.md`.

pub mod builtins;
pub mod dispatch;
pub mod interpreter;
pub mod resolver;
pub mod symbol;
pub mod types;

pub use interpreter::{Interpreter, Signal};
pub use resolver::resolve;
pub use symbol::Symbol;
pub use types::*;
