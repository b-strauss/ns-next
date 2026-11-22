//! The metadata of each body the debugger asked about, compiled again once
//! and kept (`docs/design/debugger.md`, "Compiling the metadata again").
//! On the `Mutator`, so that the pause stress mode, which runs with no
//! debugger attached, reads through it too.

use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use ns_syntax::Span;

use super::again::{
    BodyOrigin, Member, PairedBodies, compile_level, compile_level_in, member_code,
};
use super::{BodyMetadata, address};
use crate::bytecode::CompiledCode;
use crate::compiler::Lowering;
use crate::heap::MixinId;
use crate::interpreter::Mutator;
use crate::mixin::{DeclarationText, Mixin};

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
    /// Where each body a trace described stands as it was compiled
    /// (`own_placement`), keyed by the body's address, held weakly as
    /// `bodies` is.
    placements: HashMap<usize, PlacementEntry>,
    /// How many placements may be kept before the dead ones are dropped:
    /// twice what lived at the last drop, so that the drops cost as much
    /// as the inserts, and no more.
    placements_prune_at: usize,
    /// Each level compiled again from the module's current text for the
    /// placements of one trace flush, by its running side and lowering,
    /// so that the families of one level described together share the
    /// compile; emptied at the flush's end (`end_placements`), since its
    /// pairs hold every running body of the level.
    levels: Vec<SharedLevel>,
}

/// A level compiled again from the module's current text, and the side it
/// was compiled for.
struct SharedLevel {
    running: Rc<Mixin>,
    lowering: Lowering,
    level: Rc<(super::again::LevelAgain, Mixin)>,
}

/// The fewest placements the cache keeps before it drops dead ones.
const FIRST_PRUNE: usize = 1024;

/// Where a body stands as it was compiled: its metadata with every span in
/// the text its version was read in, and where the module's current text
/// holds the body's own source, if it does -- an edit that left the body
/// alone moves it there, by as much as the text before its member grew.
pub(crate) struct OwnPlacement {
    /// `None` where the body compiled again from its own source pairs
    /// with none.
    pub(crate) metadata: Option<Rc<PlacedMetadata>>,
    /// The module's current text and the body's source in it.
    pub(crate) current: Option<(Rc<str>, Span)>,
}

/// A body's statements and sends, each span in the text the body's
/// version was read in, `None` for one that text has no place for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlacedMetadata {
    pub(crate) statements: Vec<PlacedStatement>,
    pub(crate) sends: Vec<PlacedSend>,
}

/// A statement's instructions and its source's place, if it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlacedStatement {
    pub(crate) instructions: std::ops::Range<u32>,
    pub(crate) span: Option<Span>,
}

/// A send's index, site, and the places of its call and its selector's
/// tokens, each where it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlacedSend {
    pub(crate) index: u32,
    pub(crate) call_site: Option<crate::bytecode::CallSite>,
    pub(crate) call: Option<Span>,
    pub(crate) selector: Vec<Option<Span>>,
}

struct PlacementEntry {
    code: Weak<CompiledCode>,
    placement: Rc<OwnPlacement>,
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
        self.placements.clear();
        self.levels.clear();
    }

    /// Forgets the levels the placements of a flush compiled again: their
    /// pairs hold every running body of their level, which nothing else
    /// may keep alive past the flush.
    pub(crate) fn end_placements(&mut self) {
        self.levels.clear();
    }

    /// How many levels the placements of the current flush compiled.
    #[cfg(test)]
    pub(super) fn shared_levels(&self) -> usize {
        self.levels.len()
    }

    /// The level of `running` compiled again from its module's current
    /// text under `lowering`, shared with the placements of the same
    /// flush.
    fn current_level(
        mutator: &mut Mutator<'_>,
        running: &Rc<Mixin>,
        lowering: Lowering,
    ) -> Option<Rc<(super::again::LevelAgain, Mixin)>> {
        if let Some(shared) = mutator
            .debug_metadata
            .levels
            .iter()
            .find(|shared| Rc::ptr_eq(&shared.running, running) && shared.lowering == lowering)
        {
            return Some(Rc::clone(&shared.level));
        }
        let level = Rc::new(compile_level_in(mutator, running, lowering, None)?);
        mutator.debug_metadata.levels.push(SharedLevel {
            running: Rc::clone(running),
            lowering,
            level: Rc::clone(&level),
        });
        Some(level)
    }

    /// Where `code`, which `origin` names, stands as it was compiled
    /// (`OwnPlacement`), its family's computed on a miss; `None` for a
    /// factory's body, which has no source, a version without a text of
    /// its own, and a body whose module no longer holds the member.
    ///
    /// Every span counts in the text the body's version was read in,
    /// which the runtime does not keep whole: the family root's own source
    /// is laid where the member stands in the module's current text -- the
    /// current text itself where it holds that source there -- and the
    /// level compiled again from that text, so that the metadata is
    /// collected from the body's own source, laid out as it was compiled,
    /// and moved from where it stands in that text to the root's own span.
    /// A span outside the root's source -- a full initializer's statements
    /// of its superclass and `<:` clauses -- moves by the same distance
    /// where the version's text holds the same bytes there, and has no
    /// place where it does not (`moved_into`).
    pub(crate) fn own_placement(
        mutator: &mut Mutator<'_>,
        code: &Rc<CompiledCode>,
        origin: &BodyOrigin,
    ) -> Option<Rc<OwnPlacement>> {
        if let Some(entry) = mutator.debug_metadata.placements.get(&address(code))
            && entry
                .code
                .upgrade()
                .is_some_and(|alive| Rc::ptr_eq(&alive, code))
        {
            return Some(Rc::clone(&entry.placement));
        }
        let cache = &mut mutator.debug_metadata;
        if cache.placements.len() >= cache.placements_prune_at.max(FIRST_PRUNE) {
            cache
                .placements
                .retain(|_, entry| entry.code.strong_count() > 0);
            cache.placements_prune_at = 2 * cache.placements.len();
        }
        let root = member_code(&origin.mixin, &origin.member)?;
        let version = Rc::clone(origin.mixin.text.as_ref()?);
        let own_root = version.slice(root.source)?.to_string();
        let own_length = u32::try_from(own_root.len()).ok()?;
        let family = family_of(&root);
        let mut found: HashMap<usize, Rc<PlacedMetadata>> = HashMap::new();
        let mut current = None;
        for lowering in lowerings(mutator.lowering) {
            let Some(shared) = Cache::current_level(mutator, &origin.mixin, lowering) else {
                break;
            };
            let (level, fresh) = &*shared;
            let fresh_side = if origin.mixin.is_class_side() {
                fresh.class_side.as_deref()
            } else {
                Some(fresh)
            };
            let Some(standing) = fresh_side
                .and_then(|side| member_code(side, &origin.member))
                .map(|fresh_root| fresh_root.source)
            else {
                break;
            };
            let text = Rc::clone(&level.text);
            let holds =
                text.get(standing.start as usize..standing.end as usize) == Some(own_root.as_str());
            current = holds.then(|| (Rc::clone(&text), standing));
            let patched_level;
            let (pairs, compiled) = if holds {
                (&level.pairs, &level.text)
            } else {
                let patched = format!(
                    "{}{own_root}{}",
                    text.get(..standing.start as usize)?,
                    text.get(standing.end as usize..)?
                );
                match compile_level_in(mutator, &origin.mixin, lowering, Some(patched)) {
                    Some((patched, _)) => {
                        patched_level = patched;
                        (&patched_level.pairs, &patched_level.text)
                    }
                    None => continue,
                }
            };
            let region = Span::new(standing.start, standing.start + own_length);
            let mut complete = true;
            for (body, metadata) in pairs {
                if !family.contains(&address(body)) {
                    continue;
                }
                let moved = metadata
                    .as_ref()
                    .map(|metadata| moved_into(metadata, region, root.source, compiled, &version));
                complete &= moved.is_some();
                if let Some(moved) = moved {
                    found.entry(address(body)).or_insert_with(|| Rc::new(moved));
                }
            }
            if complete {
                break;
            }
        }
        let mut pending = vec![Rc::clone(&root)];
        while let Some(body) = pending.pop() {
            pending.extend(
                body.body()
                    .created_blocks()
                    .map(|block| Rc::clone(block.code)),
            );
            let current = current.as_ref().and_then(|(text, standing)| {
                Some((
                    Rc::clone(text),
                    moved_span(body.source, root.source, standing.start)?,
                ))
            });
            mutator.debug_metadata.placements.insert(
                address(&body),
                PlacementEntry {
                    code: Rc::downgrade(&body),
                    placement: Rc::new(OwnPlacement {
                        metadata: found.get(&address(&body)).cloned(),
                        current,
                    }),
                },
            );
        }
        let entry = mutator.debug_metadata.placements.get(&address(code))?;
        Some(Rc::clone(&entry.placement))
    }

    /// The text of the program module whose root is `root`, as the cache
    /// shares it: the text kept for the root when it is `text`, else
    /// `text`, kept from here on. Compared rather than trusted, since a
    /// full collection may reclaim a root's id for another module.
    pub(crate) fn module_text(&mut self, root: MixinId, text: String) -> Rc<str> {
        if let Some(kept) = self.texts.get(&root)
            && **kept == *text
        {
            return Rc::clone(kept);
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

/// The addresses of `root` and every block it creates, at any depth: a
/// worklist, since an image's body chooses how deep its blocks nest.
fn family_of(root: &Rc<CompiledCode>) -> HashSet<usize> {
    let mut family = HashSet::new();
    let mut pending = vec![Rc::clone(root)];
    while let Some(body) = pending.pop() {
        if family.insert(address(&body)) {
            pending.extend(
                body.body()
                    .created_blocks()
                    .map(|block| Rc::clone(block.code)),
            );
        }
    }
    family
}

/// `span`, which lies within `from`, moved to the same place in a copy of
/// `from`'s bytes that starts at `start`; `None` for a span outside `from`,
/// whose text the copy does not repeat.
pub(crate) fn moved_span(span: Span, from: Span, start: u32) -> Option<Span> {
    if span.start < from.start || span.end > from.end {
        return None;
    }
    Some(Span::new(
        span.start - from.start + start,
        span.end - from.start + start,
    ))
}

/// `metadata`, compiled from `compiled` where `region` holds the family
/// root's own source, with its statements' and sends' spans moved to
/// `own`, the root's own span in `version`, its version's text.
///
/// A span outside `region` is one of the declaration's own, outside the
/// body's source: a full initializer's statements of the superclass and
/// of each `<:` clause stand at the clauses, or at the declaration's name
/// where it writes none (`lower_instance_initialization_prologue`). It
/// moves by the region's own distance where the version's text holds the
/// same bytes there, its true place in that text, and has no place where
/// it does not -- an edit between the clause and the body moved them
/// apart, or the place lies outside the declaration's text or across a
/// hole of it --, never a place made up, which a reader would decorate.
/// Every statement and send is kept either way, so that the body is
/// described with all of them wherever it runs.
fn moved_into(
    metadata: &BodyMetadata,
    region: Span,
    own: Span,
    compiled: &str,
    version: &DeclarationText,
) -> PlacedMetadata {
    let outside = |span: Span| -> Option<Span> {
        let shift = |offset: u32| -> Option<u32> {
            (i64::from(offset) - i64::from(region.start) + i64::from(own.start))
                .try_into()
                .ok()
        };
        let shifted = Span::new(shift(span.start)?, shift(span.end)?);
        let held = version.slice(shifted)?;
        (Some(held) == compiled.get(span.start as usize..span.end as usize)).then_some(shifted)
    };
    let moved = |span: Span| moved_span(span, region, own.start).or_else(|| outside(span));
    PlacedMetadata {
        statements: metadata
            .statements
            .iter()
            .map(|statement| PlacedStatement {
                instructions: statement.instructions.clone(),
                span: moved(statement.span),
            })
            .collect(),
        sends: metadata
            .sends
            .iter()
            .map(|send| PlacedSend {
                index: send.index,
                call_site: send.call_site,
                call: moved(send.call),
                selector: send.selector.iter().map(|token| moved(*token)).collect(),
            })
            .collect(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The text a level was compiled again from: a class's name outside
    /// its initializer's own source, which `region()` covers.
    const COMPILED: &str = "class Ball = ( | slot ::= 1 + 2. | )";

    /// Where the initializer's own source stands in `COMPILED`.
    fn region() -> Span {
        span_of(COMPILED, "( | slot ::= 1 + 2. | )")
    }

    /// `part`'s span in `text`.
    fn span_of(text: &str, part: &str) -> Span {
        let start =
            u32::try_from(text.find(part).expect("the part is in the text")).expect("a short text");
        Span::new(
            start,
            start + u32::try_from(part.len()).expect("a short part"),
        )
    }

    /// Metadata with one statement at the class's name, outside the
    /// region, as a full initializer's superclass statement stands, one at
    /// the slot inside it, and a send whose call lies inside and whose
    /// receiver's selector token lies outside.
    fn metadata() -> BodyMetadata {
        BodyMetadata {
            source: region(),
            statements: vec![
                super::super::StatementEntry {
                    instructions: 0..1,
                    span: span_of(COMPILED, "Ball"),
                },
                super::super::StatementEntry {
                    instructions: 1..4,
                    span: span_of(COMPILED, "slot ::= 1 + 2"),
                },
            ]
            .into(),
            sends: vec![super::super::SendEntry {
                index: 2,
                call_site: None,
                receiver: None,
                selector: vec![span_of(COMPILED, "+"), span_of(COMPILED, "Ball")].into(),
                call: span_of(COMPILED, "1 + 2"),
                parenthesize_receiver: false,
                setter: false,
            }]
            .into(),
            block_sends: Vec::new().into(),
            variables: Vec::new().into(),
            factory_pattern: None,
            header_pattern: None,
        }
    }

    /// The places `moved_into` gives the outside statement and the inside
    /// one, the version's text `text`, its declaration `range` and its
    /// `holes`, the initializer's own source standing at `own`.
    fn places(
        text: &str,
        range: Span,
        holes: &[Span],
        own: Span,
    ) -> (Option<Span>, Option<Span>, PlacedSend) {
        let version = DeclarationText::cut(text, range, holes).expect("the declaration's text");
        let placed = moved_into(&metadata(), region(), own, COMPILED, &version);
        (
            placed.statements[0].span,
            placed.statements[1].span,
            placed.sends[0].clone(),
        )
    }

    /// A span inside the body's source moves with it; one outside moves by
    /// the same distance where the version's text holds the same bytes
    /// there, its true place; and one whose bytes differ there, whose place
    /// lies outside the declaration's text, or across a hole in it, has no
    /// place -- never a made-up one -- while every statement and send is
    /// kept.
    #[test]
    fn a_span_outside_the_body_moves_only_onto_the_same_bytes() {
        // An edit above the class moved the whole declaration by three bytes.
        let moved = format!("// {COMPILED}");
        let whole = Span::new(0, u32::try_from(moved.len()).expect("a short text"));
        let own = Span::new(region().start + 3, region().end + 3);
        let (outside, inside, send) = places(&moved, whole, &[], own);
        assert_eq!(
            outside.map(|span| &moved[span.start as usize..span.end as usize]),
            Some("Ball")
        );
        assert_eq!(
            inside.map(|span| &moved[span.start as usize..span.end as usize]),
            Some("slot ::= 1 + 2")
        );
        assert_eq!(
            send.call
                .map(|span| &moved[span.start as usize..span.end as usize]),
            Some("1 + 2")
        );
        assert_eq!(send.selector.len(), 2);
        assert!(send.selector.iter().all(Option::is_some));

        // The class renamed between its name and the body: the bytes differ.
        let renamed = format!("// {}", COMPILED.replace("Ball", "Bowl"));
        let (outside, inside, send) = places(&renamed, whole, &[], own);
        assert_eq!(outside, None);
        assert!(inside.is_some());
        assert_eq!(send.selector[1], None);
        assert!(send.selector[0].is_some() && send.call.is_some());

        // The declaration's text begins after the name.
        let (outside, inside, _) = places(&moved, Span::new(own.start, whole.end), &[], own);
        assert_eq!(outside, None);
        assert!(inside.is_some());

        // A hole -- a nested declaration's text -- covers the name.
        let name = span_of(&moved, "Ball");
        let (outside, inside, _) = places(
            &moved,
            whole,
            &[Span::new(name.start - 1, name.end + 1)],
            own,
        );
        assert_eq!(outside, None);
        assert!(inside.is_some());
    }

    /// A span moves with the bytes it lies in to where a copy of them
    /// starts; a span outside them, whose text the copy does not repeat,
    /// does not move at all.
    #[test]
    fn a_span_moves_only_within_its_bytes() {
        let from = Span::new(40, 60);
        assert_eq!(
            moved_span(Span::new(45, 50), from, 10),
            Some(Span::new(15, 20))
        );
        assert_eq!(moved_span(from, from, 10), Some(Span::new(10, 30)));
        assert_eq!(moved_span(Span::new(5, 8), from, 10), None);
        assert_eq!(moved_span(Span::new(55, 61), from, 10), None);
    }
}
