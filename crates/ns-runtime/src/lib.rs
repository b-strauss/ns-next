#![warn(clippy::all)]
// The brand needs no unsafe code: `World::enter` (`heap/world.rs`) hands the
// step state to a closure the compiler checks for every `'gc`.
#![forbid(unsafe_code)]

//! Name resolution, object model, bytecode compiler and VM for the
//! Newspeak3 language, including nested classes, mixin
//! applications, and the source-free image format.
//! See `docs/design/` -- start at its `README.md`.

mod arbitrary_integer;
pub mod builtins;
pub mod bytecode;
pub mod code_points;
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

pub use arbitrary_integer::ArbitraryInteger;
pub use compiler::Lowering;

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
pub use heap::{
    ActivationHeader, ActivationReference, ApplicationIndex, ClassReference, CodeId,
    CollectorStatistics, FrameRecord, FrameReference, Heap, HeapStatistics, MixinId, NewClass,
    NewClosure, ObjectReference, ObjectShape, RegisterBase, Root,
};
pub use interpreter::core_library::MANIFEST_SOURCE;
pub use interpreter::{
    CacheStatistics, CachedTarget, CachedTargetKind, CallSiteState, HostError, HostResult,
    HostSignal, Interpreter, MethodSide, Mutator,
};
pub use mixin::{DispatchId, DispatchIds, Mixin, MixinApplication};
pub use resolver::{resolve, resolve_for_editor};
pub use symbol::Symbol;
pub use types::*;
