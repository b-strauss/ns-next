//! The metadata of each body the debugger asked about, compiled again once
//! and kept (`docs/design/debugger.md`, "Compiling the metadata again").
//! On the `Mutator`, so that the pause stress mode, which runs with no
//! debugger attached, reads through it too.

use std::collections::HashMap;
use std::rc::{Rc, Weak};

use super::again::{compile_level, BodyOrigin, Member, PairedBodies};
use super::BodyMetadata;
use crate::bytecode::CompiledCode;
use crate::compiler::Lowering;
use crate::interpreter::Mutator;

/// Each body's metadata, `None` for a body that pairs with none, keyed by
/// the body's address.
#[derive(Default)]
pub(crate) struct Cache {
    bodies: HashMap<usize, Entry>,
}

/// One body's entry. The body is held weakly: a strong hold would keep
/// every body the debugger ever looked at alive, and with it its inline
/// caches' family (`InlineCaches::forget_families` frees a family only
/// once its body is held by the family alone). A `Weak` still keeps the
/// body's allocation, so no other body takes its address while the entry
/// lives, and a dead entry is forgotten at the next miss.
struct Entry {
    code: Weak<CompiledCode>,
    metadata: Option<Rc<BodyMetadata>>,
}

impl Cache {
    /// The metadata of `code`, which `origin` names, compiling its level
    /// again on a miss: under the interpreter's lowering, then under the
    /// other one when the first left a body of the level unpaired (an
    /// image's code, which either may have compiled: decision 5); a body
    /// that pairs with none is cached as `None`. A factory's body no mixin
    /// holds -- a host's `instantiate` builds its own -- takes the
    /// header's metadata when it is the factory body of the header's
    /// arity.
    pub(crate) fn metadata(
        mutator: &mut Mutator<'_>,
        code: &Rc<CompiledCode>,
        origin: &BodyOrigin,
    ) -> Option<Rc<BodyMetadata>> {
        if let Some(entry) = mutator.debug_metadata.bodies.get(&address(code)) {
            return entry.metadata.clone();
        }
        mutator.debug_metadata.forget_dead();
        let first = mutator.lowering;
        let other = match first {
            Lowering::Optimized => Lowering::Reference,
            Lowering::Reference => Lowering::Optimized,
        };
        for lowering in [first, other] {
            // No text, or text that holds no such declaration, is the same
            // under either lowering.
            let Some(level) = compile_level(mutator, &origin.mixin, lowering) else {
                break;
            };
            let complete = mutator.debug_metadata.fill(level.pairs);
            if matches!(origin.member, Member::Factory)
                && !mutator.debug_metadata.bodies.contains_key(&address(code))
            {
                let metadata = level.factory.metadata_of(code);
                mutator
                    .debug_metadata
                    .fill(vec![(Rc::clone(code), metadata)]);
            }
            if complete {
                break;
            }
        }
        let cache = &mut mutator.debug_metadata;
        cache
            .bodies
            .entry(address(code))
            .or_insert_with(|| Entry {
                code: Rc::downgrade(code),
                metadata: None,
            })
            .metadata
            .clone()
    }

    /// Forgets every body's metadata: an install changes the module's
    /// text, which every span counts in, and may replace what the bodies
    /// pair with.
    pub(crate) fn clear(&mut self) {
        self.bodies.clear();
    }

    /// Records `pairs`, a body's tables never replaced by `None` nor by
    /// another lowering's; answers whether every body of them paired.
    fn fill(&mut self, pairs: PairedBodies) -> bool {
        let mut complete = true;
        for (code, metadata) in pairs {
            complete &= metadata.is_some();
            let entry = self.bodies.entry(address(&code)).or_insert_with(|| Entry {
                code: Rc::downgrade(&code),
                metadata: None,
            });
            if entry.metadata.is_none() {
                entry.metadata = metadata;
            }
        }
        complete
    }

    /// Drops the entry of every body nothing else holds.
    fn forget_dead(&mut self) {
        self.bodies.retain(|_, entry| entry.code.strong_count() > 0);
    }
}

/// A body's address, which names it while it lives.
fn address(code: &Rc<CompiledCode>) -> usize {
    Rc::as_ptr(code).addr()
}
