#![warn(clippy::all)]
// The brand needs no unsafe code: `World::enter` (`heap/world.rs`) hands the
// step state to a closure the compiler checks for every `'gc`.
#![forbid(unsafe_code)]
// Without an encoder no body is compiled (`jit::available`), so what only
// machine code reaches -- the leaves' decisions, the words and cells it
// reads -- goes unused there; the JIT's build, which every gate's suite
// runs, holds the crate to its dead code.
#![cfg_attr(
    not(ns_jit),
    allow(
        dead_code,
        reason = "what only machine code reaches is unused where no body is compiled"
    )
)]

//! Name resolution, object model, bytecode compiler and VM for the
//! Newspeak3 language, including nested classes, mixin
//! applications, and the source-free image format.
//! See `docs/design/` -- start at its `README.md`.

mod arbitrary_integer;
pub mod builtins;
pub mod bytecode;
pub mod code_points;
pub mod compiler;
pub mod debugger;
pub mod dispatch;
pub mod hash;
pub mod heap;
pub mod image;
pub mod interpreter;
mod jit;
pub mod mixin;
pub(crate) mod numeric;
pub mod platform;
pub mod resolver;
pub mod shipped_code;
pub mod symbol;
pub(crate) mod trace;
pub mod types;
pub mod vm;

pub use arbitrary_integer::ArbitraryInteger;
pub use compiler::Lowering;
// The debugger's `FrameKind` stays in `debugger`: the crate root's is the
// activation kind of `types`.
pub use debugger::{
    BreakableStatement, BreakpointId, BreakpointState, ClassName, Command, DebugControl,
    DebugEvents, DebugRemote, Event, FrameDescription, FrameLocation, HitCondition, LineBreakpoint,
    MethodBreakpoint, MethodBreakpointState, ModuleTextAnswer, SendLocation, StopReason,
    ValueDescription, ValueId, Variable, VariableValue,
};

/// Test helper: turns one test body into a module holding an `optimized` and
/// a `reference` test, each binding `lowering` to that `Lowering` variant and
/// running the body. A loop over both lowerings whose iterations are
/// independent -- each builds its own interpreter and asserts on its own
/// result -- makes one test run twice in sequence; expanding it into two
/// tests instead lets `cargo nextest run` run them in parallel
/// (`docs/design/compiler.md`, "The harness"). Outer attributes (doc
/// comments, `#[cfg(...)]`, `#[ignore]`, `#[should_panic]`) are attached to
/// both generated tests, and `use super::*;` inside the generated module
/// reaches whatever the enclosing test file or module brought into scope.
#[macro_export]
#[doc(hidden)]
macro_rules! for_each_lowering {
    (
        $(#[$attribute:meta])*
        fn $name:ident($lowering:ident) $body:block
    ) => {
        mod $name {
            // A test gated to debug builds can leave the import unused in
            // a release build; a debug build still reports one.
            #[cfg_attr(not(debug_assertions), allow(unused_imports, reason = "a test gated to debug builds can leave the import unused"))]
            use super::*;

            $(#[$attribute])*
            #[test]
            fn optimized() {
                let $lowering = $crate::Lowering::Optimized;
                $body
            }

            $(#[$attribute])*
            #[test]
            fn reference() {
                let $lowering = $crate::Lowering::Reference;
                $body
            }
        }
    };
}
pub use code_points::CodePoints;
pub use heap::limits::{Shrink, ShrinkPairs, TableFloors, TableSizes};
pub use heap::{
    ActivationHeader, ActivationReference, ApplicationIndex, ClassReference, CodeId,
    CollectorStatistics, FrameRecord, FrameReference, Heap, HeapStatistics, MixinId, NewClass,
    NewClosure, ObjectReference, ObjectShape, RegisterBase, Root, StackExhausted,
};
pub use interpreter::core_library::{
    shipped_module_named_by_text, shipped_module_source, shipped_source_files,
    shipped_sources_fingerprint, ShippedSourceFile, MANIFEST_SOURCE,
};
pub use interpreter::{
    CacheStatistics, CachedTarget, CachedTargetKind, CallSiteCell, CallSiteState, HostError,
    HostResult, HostSignal, Interpreter, MethodSide, Mutator,
};
pub use jit::JitStatistics;
#[doc(hidden)]
pub use jit::LAST_VERSION;
pub use mixin::{DispatchId, DispatchIds, Mixin, MixinApplication};
/// The record of what a test's process reaches (`ns_memory::reach`),
/// re-exported so that the crates above the runtime record their own
/// platform-dependent code without depending on the memory layer.
pub use ns_memory::reach;
pub use resolver::{resolve, resolve_for_editor, root_class_is_value};
pub use symbol::Symbol;
pub use types::*;
