//! The metadata of each body the debugger asked about, compiled again once
//! and kept (`docs/design/debugger.md`, "Compiling the metadata again").
//! On the `Mutator`, so that the pause stress mode, which runs with no
//! debugger attached, reads through it too.

use std::collections::HashMap;
use std::rc::{Rc, Weak};

use super::again::{compile_level, BodyOrigin, Member, PairedBodies};
use super::{address, BodyMetadata};
use crate::bytecode::CompiledCode;
use crate::compiler::Lowering;
use crate::heap::MixinId;
use crate::interpreter::Mutator;
use crate::mixin::Mixin;

/// Each body's metadata, `None` for a body that pairs with none, keyed by
/// the body's address, and the text of each program module a level was
/// compiled from, keyed by the module's root.
#[derive(Default)]
pub(crate) struct Cache {
    bodies: HashMap<usize, Entry>,
    /// One text per module, which every level of it compiled again and
    /// every entry of their bodies share, rather than a copy per level.
    /// A shipped module's text is the binary's, shared already
    /// (`shipped_module_text`).
    texts: HashMap<MixinId, Rc<str>>,
}

/// One body's entry, its metadata beside the module's text the metadata
/// was compiled from, which every span of it counts in: what names a
/// returned value's call slices, without assembling the text again or
/// looking the module up by name, which two loaded modules may share.
/// The body is held weakly: a strong hold would keep every body the
/// debugger ever looked at alive, and with it its inline caches' family
/// (`InlineCaches::forget_families` frees a family only once its body is
/// held by the family alone). A `Weak` still keeps the body's allocation,
/// so no other body takes its address while the entry lives, and a dead
/// entry is forgotten at the next miss.
struct Entry {
    code: Weak<CompiledCode>,
    metadata: Option<(Rc<BodyMetadata>, Rc<str>)>,
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
        Cache::metadata_with_text(mutator, code, origin).map(|(metadata, _)| metadata)
    }

    /// `metadata`, with the module's text it was compiled from.
    pub(crate) fn metadata_with_text(
        mutator: &mut Mutator<'_>,
        code: &Rc<CompiledCode>,
        origin: &BodyOrigin,
    ) -> Option<(Rc<BodyMetadata>, Rc<str>)> {
        if let Some(entry) = mutator.debug_metadata.bodies.get(&address(code)) {
            return entry.metadata.clone();
        }
        mutator.debug_metadata.forget_dead();
        for lowering in lowerings(mutator.lowering) {
            // No text, or text that holds no such declaration, is the same
            // under either lowering.
            let Some(level) = compile_level(mutator, &origin.mixin, lowering) else {
                break;
            };
            let complete = mutator.debug_metadata.fill(level.pairs, &level.text);
            if matches!(origin.member, Member::Factory)
                && !mutator.debug_metadata.bodies.contains_key(&address(code))
            {
                let metadata = level.factory.metadata_of(code);
                mutator
                    .debug_metadata
                    .fill(vec![(Rc::clone(code), metadata)], &level.text);
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

    /// Every body of the level of the instance side `mixin` -- its own,
    /// its class side's and every block they create -- with its metadata,
    /// compiled again as `metadata` compiles it: what a breakpoint is
    /// resolved against. Empty when the level has no text, or its text
    /// does not hold the declaration.
    pub(crate) fn level(mutator: &mut Mutator<'_>, mixin: &Rc<Mixin>) -> PairedBodies {
        mutator.debug_metadata.forget_dead();
        let mut bodies = Vec::new();
        for lowering in lowerings(mutator.lowering) {
            let Some(level) = compile_level(mutator, mixin, lowering) else {
                break;
            };
            if bodies.is_empty() {
                bodies = level
                    .pairs
                    .iter()
                    .map(|(code, _)| Rc::clone(code))
                    .collect();
            }
            if mutator.debug_metadata.fill(level.pairs, &level.text) {
                break;
            }
        }
        let cache = &mutator.debug_metadata;
        bodies
            .into_iter()
            .map(|code| {
                let metadata = cache
                    .bodies
                    .get(&address(&code))
                    .and_then(|entry| entry.metadata.as_ref())
                    .map(|(metadata, _)| Rc::clone(metadata));
                (code, metadata)
            })
            .collect()
    }

    /// Forgets every body's metadata and every module's text: an install
    /// changes the module's text, which every span counts in, and may
    /// replace what the bodies pair with.
    pub(crate) fn clear(&mut self) {
        self.bodies.clear();
        self.texts.clear();
    }

    /// The text of the program module whose root is `root`, as the cache
    /// shares it: the text kept for the root when it is `text`, else
    /// `text`, kept from here on. Compared rather than trusted, since a
    /// full collection may reclaim a root's id for another module.
    pub(super) fn module_text(&mut self, root: MixinId, text: String) -> Rc<str> {
        if let Some(kept) = self.texts.get(&root) {
            if **kept == *text {
                return Rc::clone(kept);
            }
        }
        let text: Rc<str> = Rc::from(text);
        self.texts.insert(root, Rc::clone(&text));
        text
    }

    /// Records `pairs`, compiled from `text`, a body's tables never
    /// replaced by `None` nor by another lowering's; answers whether every
    /// body of them paired.
    fn fill(&mut self, pairs: PairedBodies, text: &Rc<str>) -> bool {
        let mut complete = true;
        for (code, metadata) in pairs {
            complete &= metadata.is_some();
            let entry = self.bodies.entry(address(&code)).or_insert_with(|| Entry {
                code: Rc::downgrade(&code),
                metadata: None,
            });
            if entry.metadata.is_none() {
                entry.metadata = metadata.map(|metadata| (metadata, Rc::clone(text)));
            }
        }
        complete
    }

    /// Drops the entry of every body nothing else holds.
    fn forget_dead(&mut self) {
        self.bodies.retain(|_, entry| entry.code.strong_count() > 0);
    }
}

/// The lowerings a level is compiled again under, in order: the
/// interpreter's, then the other, for an image's code, which either may
/// have compiled (decision 5).
fn lowerings(first: Lowering) -> [Lowering; 2] {
    let other = match first {
        Lowering::Optimized => Lowering::Reference,
        Lowering::Reference => Lowering::Optimized,
    };
    [first, other]
}
