//! A full collection's reclamation of the ids nothing names any more
//! (`Mutator::reclaim_ids`; `docs/design/object-model.md`, "An application
//! has two ids").

use std::collections::HashSet;
use std::rc::{Rc, Weak};

use super::Mutator;
use crate::hash::{IdentityBuildHasher, IdentityMap};
use crate::heap::reclaim::{mixin_row_number, BitSet, FreedIds, GatheredIds};
use crate::heap::ApplicationIndex;
use crate::mixin::{DispatchId, Mixin};

impl Mutator<'_> {
    /// Frees every application nothing names any more, with its index and
    /// its guard id, and vacates exactly the cache entries that named one,
    /// so that the next application minted at a freed index or with a
    /// freed guard is never mistaken for the dead one; then every mixin id
    /// and code id nothing names any more, and the cache range of every
    /// family nothing runs any more. Runs in a full
    /// collection between marking and compaction (`World::collect`):
    /// `gathered` is what the marked objects name, and nothing here moves
    /// or allocates an object.
    ///
    /// The steps run in order, each releasing the `Rc`s the next one
    /// counts: the applications, the caches that hold them, the mixins
    /// the applications applied, the code ids, and the feedback ranges.
    pub(crate) fn reclaim_ids(&mut self, mut gathered: GatheredIds) {
        let stack = self.heap.stack_roots();
        // A running activation names its body by id, beside the marked
        // closures and frames: step 3 asks which bodies run, step 4 frees
        // the rest.
        gathered.running_codes(&stack.codes);
        // Step 1: applications. Live: what a marked class object names,
        // what an activation was entered at, the application whose guard
        // an activation recorded -- a receiver forwarded under it must
        // never meet the guard reused -- and, transitively, superclass
        // links and successors.
        let rows = self.heap.application_rows();
        let mut live = vec![false; rows];
        let mut work: Vec<ApplicationIndex> = (0..rows)
            .filter(|row| gathered.applications.contains(row_number(*row)))
            .map(application_index)
            .collect();
        work.extend(
            stack
                .entered_levels
                .iter()
                .copied()
                .filter(|level| *level != ApplicationIndex::UNREGISTERED),
        );
        let recorded: HashSet<DispatchId, IdentityBuildHasher> = stack
            .recorded_guards
            .iter()
            .copied()
            .filter(|guard| *guard != DispatchId::NONE)
            .collect();
        if !recorded.is_empty() {
            for row in 0..rows {
                let index = application_index(row);
                if self.heap.application(index).is_some_and(|application| {
                    recorded.contains(&application.dispatch_ids.instances)
                }) {
                    work.push(index);
                }
            }
        }
        while let Some(index) = work.pop() {
            let Some(flag) = live.get_mut(index.index()) else {
                continue;
            };
            if std::mem::replace(flag, true) {
                continue;
            }
            if let Some(application) = self.heap.application(index) {
                if let Some(superclass) = &application.superclass {
                    work.push(superclass.index());
                }
            }
            if let Some(successor) = self.heap.successor(index) {
                work.push(successor);
            }
        }
        self.heap
            .retain_successors(|old| live.get(old.index()).copied().unwrap_or(false));
        let mut freed = FreedIds::default();
        let mut dropped = Vec::new();
        for (row, is_live) in live.iter().enumerate() {
            let index = application_index(row);
            if *is_live || self.heap.application(index).is_none() {
                continue;
            }
            let application = self.heap.free_application(index);
            let guard = application.dispatch_ids.instances;
            if guard != DispatchId::NONE {
                self.dispatch_ids.free(guard);
                freed.guards.insert(guard);
            }
            freed.applications.insert(index);
            dropped.push(application);
        }
        // Step 2: the caches, only when something was freed.
        if !freed.is_empty() {
            self.applications.retain(|(_, superclass), application| {
                !freed.applications.contains(&application.index())
                    && superclass.is_none_or(|superclass| !freed.applications.contains(&superclass))
            });
            self.caches.forget_freed(&freed);
        }
        // No other holder may outlive the scrub: an index freed while an
        // `Rc` still names it would alias the next application minted
        // there. A dropped application is held once by `dropped` and once
        // by each dropped application whose superclass it is -- a live one
        // never names a dead superclass -- so its count is checked against
        // exactly those links.
        if cfg!(debug_assertions) {
            let mut superclass_links: IdentityMap<ApplicationIndex, usize> = IdentityMap::default();
            for application in &dropped {
                if let Some(superclass) = &application.superclass {
                    *superclass_links.entry(superclass.index()).or_default() += 1;
                }
            }
            for application in &dropped {
                let links = superclass_links
                    .get(&application.index())
                    .copied()
                    .unwrap_or(0);
                assert_eq!(
                    Rc::strong_count(application),
                    1 + links,
                    "internal: a reclaimed application is held outside the tables and caches"
                );
            }
        }
        drop(dropped);
        self.heap.truncate_application_rows();
        self.reclaim_mixins(&gathered);
        // Step 4: codes. The marked closures and frames and the running
        // activations are every holder of one.
        self.heap.free_unnamed_codes(&gathered.codes);
        // Step 5: feedback ranges. Whether or not anything above was freed:
        // a body steps 3 and 4 let go of can end a family on its own.
        let mut live_bases = gathered.feedback_bases;
        live_bases.extend(stack.feedback_bases.iter().map(|base| base.raw()));
        self.caches.forget_families(&live_bases);
    }

    /// Step 3: frees every module's tree of mixin ids nothing names any
    /// more, then the superseded versions only the table holds and no
    /// closure, frame or activation runs code of. A tree
    /// lives while any of its ids is named by a live application, a marked
    /// class object's applied clause or a reified mixin a program can still
    /// compare, or any of its versions is held outside the tables: a
    /// holder that presents a version again must find it under the id it
    /// had, which is why a tree dies whole -- a live mixin names the ids of
    /// the mixins it is declared in and of those declared in it.
    fn reclaim_mixins(&mut self, gathered: &GatheredIds) {
        let trees = self.heap.module_trees();
        let applied = self.heap.applied_mixin_ids();
        let reified = self.heap.reified_mixin_ids();
        let held = self.heap.held_mixin_ids(&trees, &self.metaclass_mixins);
        let mut freed = BitSet::default();
        let mut any_freed = false;
        for tree in &trees {
            let lives = tree.iter().any(|id| {
                let row = mixin_row_number(*id);
                applied.contains(row)
                    || gathered.applied_clauses.contains(row)
                    || reified.contains(row)
                    || held.contains(row)
            });
            if lives {
                continue;
            }
            for id in tree {
                freed.insert(mixin_row_number(*id));
                // A memo of the tree, keyed by an id about to be minted
                // again for another mixin.
                self.metaclass_mixins.remove(id);
            }
            any_freed = true;
        }
        if any_freed {
            debug_assert!(
                self.applications
                    .keys()
                    .all(|(mixin, _)| !freed.contains(mixin_row_number(*mixin))),
                "internal: a cached application's mixin is named by the application"
            );
            let dropped = self.heap.free_mixins(&freed);
            // Nothing outside the tables may still hold a dropped version:
            // it would find its id minted again for another mixin.
            let watched: Vec<Weak<Mixin>> = if cfg!(debug_assertions) {
                dropped.iter().map(Rc::downgrade).collect()
            } else {
                Vec::new()
            };
            drop(dropped);
            assert!(
                watched.iter().all(|version| version.upgrade().is_none()),
                "internal: a freed mixin is held outside the tables"
            );
        }
        // Before step 4 frees the code ids that say which bodies run.
        let running = self.heap.running_bodies(&gathered.codes);
        self.heap.drop_superseded(&running);
        self.heap.truncate_mixin_rows();
    }
}

/// The row of the application table a `GatheredIds` bit stands for.
fn row_number(row: usize) -> u32 {
    u32::try_from(row).expect("an application row fits a u32")
}

fn application_index(row: usize) -> ApplicationIndex {
    ApplicationIndex::from_index(row).expect("an application row fits a u32")
}

#[cfg(test)]
mod tests {
    #![cfg_attr(
        not(debug_assertions),
        allow(
            dead_code,
            reason = "the helpers are read only by tests gated to debug builds"
        )
    )]

    use crate::heap::MixinId;
    use crate::interpreter::edit::{Edit, MemberEdit};
    use crate::interpreter::{HostResult, HostSignal, Interpreter, Mutator};
    use crate::mixin::Mixin;
    use crate::types::SourceOrigin;
    use crate::{Lowering, Root, Symbol};
    use indoc::indoc;
    use std::path::PathBuf;
    use std::rc::Rc;

    fn load_into(interpreter: &mut Interpreter, source: &str) -> Root {
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(source),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        interpreter.load_module(&resolved)
    }

    fn load(source: &str) -> (Interpreter, Root) {
        load_with(Lowering::Optimized, source)
    }

    fn load_with(lowering: Lowering, source: &str) -> (Interpreter, Root) {
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        let class = load_into(&mut interpreter, source);
        (interpreter, class)
    }

    fn expect_ok(result: HostResult, what: &str) -> Root {
        match result {
            Ok(root) => root,
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    fn send(interpreter: &mut Interpreter, receiver: &Root, selector: &str) -> Root {
        expect_ok(
            interpreter.send(receiver, Symbol::intern(selector), &[]),
            selector,
        )
    }

    fn send_with(
        interpreter: &mut Interpreter,
        receiver: &Root,
        selector: &str,
        arguments: &[Root],
    ) -> Root {
        expect_ok(
            interpreter.send(receiver, Symbol::intern(selector), arguments),
            selector,
        )
    }

    fn integer(interpreter: &mut Interpreter, root: &Root) -> Option<i64> {
        interpreter.enter(|mutator| mutator.fetch(root).as_small_integer())
    }

    /// The application rows and the guard ids' high water.
    fn id_counts(interpreter: &mut Interpreter) -> (usize, u32) {
        interpreter.enter(|mutator| {
            (
                mutator.heap.application_rows(),
                mutator.dispatch_ids.high_water(),
            )
        })
    }

    /// `run:` builds a chain of a hundred classes over `start`, each
    /// round's `Local` over the previous round's, and keeps none of it.
    /// Two runs over the same start would share every application through
    /// the application cache, so the test starts each run from a class of
    /// its own: only the second run's chain is new, and it must take the
    /// rows and guards the first one left.
    const CHURN: &str = indoc! {"
        Newspeak3
        'Test'
        class Churn = (
        ) (
          public class First = (
          ) (
          )
          public class Second = (
          ) (
          )
          class Holder base: superclass = (
            | public base = superclass. |
          ) (
            public class Local = base (
            ) (
            )
          )
          public run: start = (
            | current |
            current:: start.
            1 to: 100 do: [:each | current:: (Holder base: current) Local].
            ^current new
          )
        )
    "};

    /// A chain of a hundred applications, each over the previous round's
    /// class, built and dropped twice: the second chain takes the rows and
    /// the guards the collection after the first freed, so neither count
    /// grows.
    #[cfg(debug_assertions)]
    #[test]
    fn dead_applications_are_reclaimed_and_their_guards_minted_again() {
        let (mut interpreter, churn) = load(CHURN);
        let instance = expect_ok(interpreter.instantiate(&churn, &[]), "a Churn");
        let first = send(&mut interpreter, &instance, "First");
        let second = send(&mut interpreter, &instance, "Second");
        drop(send_with(&mut interpreter, &instance, "run:", &[first]));
        interpreter.collect_now();
        let counts = id_counts(&mut interpreter);
        drop(send_with(&mut interpreter, &instance, "run:", &[second]));
        interpreter.collect_now();
        assert_eq!(id_counts(&mut interpreter), counts);
        interpreter.enter(|mutator| {
            assert_eq!(mutator.heap.verify_application_ids(), Ok(()));
        });
        // Between runs no activation is live, so the stack roots nothing.
        interpreter.enter(|mutator| {
            let stack = mutator.heap.stack_roots();
            assert!(stack.entered_levels.is_empty());
            assert!(stack.recorded_guards.is_empty());
            assert!(stack.feedback_bases.is_empty());
            assert!(stack.codes.is_empty());
        });
    }

    const SUPERSEDED: &str = indoc! {"
        Newspeak3
        'Test'
        class Superseded = (
        ) (
          class Base = (
            | public zero = 10. |
          ) (
          )
          class Subject before: install after: collect = Base (
            |
              public first = install value.
              public second = 2.
              public third = collect value.
              public fourth = 4.
            |
          ) (
          )
          public run: platform = (
            | builder instance |
            builder:: (platform mirrors ClassMirror reflecting: Subject) mixin declaration
                asBuilder.
            builder instanceSide methods addFromSource: 'public added = (^3)'.
            instance:: Subject
                before: [
                  builder install.
                  1
                ]
                after: [
                  platform kernel garbageCollect.
                  3
                ].
            ^instance zero + instance first + instance second + instance third
              + instance fourth + instance added
          )
        )
    "};

    /// The application id table machine code reads
    /// (`heap/application_ids.rs`) follows the application table through
    /// registration, a collection's freeing of a hundred dead rows, the
    /// trailing ones given back, and their minting again.
    // `collect_now` is a debug build's.
    #[cfg(debug_assertions)]
    #[test]
    fn the_application_id_table_follows_the_application_table() {
        let (mut interpreter, churn) = load(CHURN);
        let verify = |interpreter: &mut Interpreter| {
            interpreter.enter(|mutator| {
                assert_eq!(mutator.heap.verify_application_ids(), Ok(()));
            });
        };
        verify(&mut interpreter);
        let instance = expect_ok(interpreter.instantiate(&churn, &[]), "a Churn");
        let first = send(&mut interpreter, &instance, "First");
        let second = send(&mut interpreter, &instance, "Second");
        drop(send_with(&mut interpreter, &instance, "run:", &[first]));
        verify(&mut interpreter);
        interpreter.collect_now();
        verify(&mut interpreter);
        drop(send_with(&mut interpreter, &instance, "run:", &[second]));
        verify(&mut interpreter);
        interpreter.collect_now();
        verify(&mut interpreter);
    }

    /// An initializer whose class an install re-applied, keeping its
    /// slots, runs on at the level it was entered at across a later full
    /// collection: the superseded application stays, so the slots after
    /// the collection are written where the code expects them. `second`,
    /// written between the install and the collection, passes the slot
    /// check's cold path and records the receiver's new guard, so that only
    /// the activation's entered level keeps the old application live; and
    /// `Base`'s slot puts `Subject`'s at base 1, so that a slot base read
    /// off a freed row would write `third` over `zero`.
    #[test]
    fn an_activation_at_a_superseded_level_runs_on_across_a_collection() {
        let (mut interpreter, class) = load(SUPERSEDED);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "a Superseded");
        let platform = interpreter.platform();
        let collections = interpreter.collector_statistics().collections;
        let answer = send_with(&mut interpreter, &instance, "run:", &[platform]);
        assert!(interpreter.collector_statistics().collections > collections);
        assert_eq!(integer(&mut interpreter, &answer), Some(23));
    }

    /// `A` is made through a fresh `Maker` each time, so that once the
    /// instances are dropped nothing holds `A`'s class object: the `Guards`
    /// instance caches `Maker`, never an `A`. `drive` leaves `probe:`
    /// monomorphic on `A`, `poly:` polymorphic on `A`, `B` and `D`, and
    /// `mega:` megamorphic, so that the global cache holds `A`'s entry.
    const GUARDS: &str = indoc! {"
        Newspeak3
        'Test'
        class Guards = (
        ) (
          class Maker = (
          ) (
            public class A = (
            ) (
              public value = (
                ^1
              )
            )
          )
          public class B = (
          ) (
            public value = (
              ^2
            )
          )
          public class C = (
          ) (
            public value = (
              ^3
            )
          )
          public class D = (
          ) (
            public value = (
              ^4
            )
          )
          public class E = (
          ) (
            public value = (
              ^5
            )
          )
          public class F = (
          ) (
            public value = (
              ^6
            )
          )
          public newA = (
            ^Maker new A new
          )
          public probe: object = (
            ^object value
          )
          public poly: object = (
            ^object value
          )
          public mega: object = (
            ^object value
          )
          public drive = (
            | a |
            a:: newA.
            1 to: 3 do: [:each | probe: a].
            poly: a.
            poly: B new.
            poly: D new.
            mega: a.
            mega: B new.
            mega: D new.
            mega: E new.
            mega: F new.
          )
        )
    "};

    /// The mixin the class object `class` applies.
    fn mixin_of(mutator: &Mutator<'_>, class: &Root) -> Rc<Mixin> {
        let class = mutator.fetch(class).as_object().expect("a class object");
        mutator
            .heap()
            .class_application(class)
            .expect("a class object")
            .mixin
            .clone()
    }

    /// The id of the mixin the class object `class` applies.
    fn id_of(interpreter: &mut Interpreter, class: &Root) -> MixinId {
        interpreter.enter(|mutator| {
            let mixin = mixin_of(mutator, class);
            mutator.heap.mixin_id(&mixin)
        })
    }

    fn install(interpreter: &mut Interpreter, id: MixinId, edit: &Edit) {
        if let Err(diagnostic) = interpreter.enter(|mutator| mutator.install_edit(id, edit)) {
            panic!("the install was refused: {diagnostic}");
        }
    }

    fn add_method(text: &str) -> Edit {
        Edit {
            header: None,
            instance_side: vec![MemberEdit::AddMethod(text.to_string())],
            class_side: Vec::new(),
        }
    }

    /// The mixin rows, the superseded versions and the code rows.
    fn table_counts(interpreter: &mut Interpreter) -> (usize, usize, usize) {
        interpreter.enter(|mutator| {
            (
                mutator.heap.mixin_rows(),
                mutator.heap.superseded_count(),
                mutator.heap.code_rows(),
            )
        })
    }

    const OUTER: &str = indoc! {"
        Newspeak3
        'Test'
        class Outer = (
          | public kept ::= nil. |
        ) (
          public class Inner = (
          ) (
            public answer = (
              ^1
            )
          )
          public make = (
            ^[ 7 ]
          )
        )
    "};

    /// A module nothing names any more gives back every row of its tree --
    /// the root, the nested class and the class sides their metaclasses
    /// registered: every row of the tree is free after the collection.
    #[cfg(debug_assertions)]
    #[test]
    fn a_dead_module_tree_is_reclaimed() {
        let (mut interpreter, outer) = load(OUTER);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let inner_class = send(&mut interpreter, &instance, "Inner");
        let inner = send(&mut interpreter, &inner_class, "new");
        drop(send(&mut interpreter, &inner, "answer"));
        // Numbers, which hold nothing.
        let outer_id = id_of(&mut interpreter, &outer);
        let inner_id = id_of(&mut interpreter, &inner_class);
        let tree = interpreter.enter(|mutator| {
            mutator
                .heap
                .module_trees()
                .into_iter()
                .find(|tree| tree.contains(&outer_id))
                .expect("the module has a tree")
        });
        assert!(tree.contains(&inner_id));
        // The two class sides besides the two instance sides.
        assert!(tree.len() >= 4, "the tree holds the class sides too");
        drop((outer, instance, inner_class, inner));
        interpreter.collect_now();
        interpreter.enter(|mutator| {
            for id in &tree {
                assert!(
                    mutator.heap.mixin(*id).is_none(),
                    "a row of the tree is free"
                );
            }
        });
    }

    /// A host's `Rc` of a module's mixin keeps the whole tree's ids: the
    /// mixin answers its id after a collection nothing else survives, and
    /// its nested declaration the same one as before.
    #[cfg(debug_assertions)]
    #[test]
    fn a_host_held_mixin_keeps_its_id() {
        let (mut interpreter, outer) = load(OUTER);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        drop(send(&mut interpreter, &instance, "Inner"));
        let (held, id, nested) = interpreter.enter(|mutator| {
            let held = mixin_of(mutator, &outer);
            let id = mutator.heap.mixin_id(&held);
            let nested = mutator.heap.nested_of(id).to_vec();
            (held, id, nested)
        });
        assert_eq!(nested.len(), 1);
        drop((outer, instance));
        interpreter.collect_now();
        interpreter.enter(|mutator| {
            assert_eq!(mutator.heap.known_mixin_id(&held), Some(id));
            assert_eq!(mutator.heap.nested_of(id), nested.as_slice());
            let inner = mutator.heap.mixin(nested[0]).expect("Inner keeps its row");
            assert_eq!(inner.name.as_str(), "Inner");
        });
    }

    /// Three edits of `Outer` leave three superseded versions of it, and of
    /// `Inner`, which the install replaced with it; once no activation,
    /// closure or class object holds one, a collection drops them all.
    #[cfg(debug_assertions)]
    #[test]
    fn superseded_versions_nothing_holds_are_dropped() {
        let (mut interpreter, outer) = load(OUTER);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let id = id_of(&mut interpreter, &outer);
        for answer in 1..=3 {
            install(
                &mut interpreter,
                id,
                &add_method(&format!("public answer = ( ^{answer} )")),
            );
            let result = send(&mut interpreter, &instance, "answer");
            assert_eq!(integer(&mut interpreter, &result), Some(answer));
        }
        assert!(table_counts(&mut interpreter).1 >= 3);
        interpreter.collect_now();
        assert_eq!(table_counts(&mut interpreter).1, 0);
        let result = send(&mut interpreter, &instance, "answer");
        assert_eq!(integer(&mut interpreter, &result), Some(3));
    }

    /// A closure made by a version three edits superseded still runs after
    /// a collection: its body lives on with the closure, though no
    /// superseded version is kept for it.
    #[cfg(debug_assertions)]
    #[test]
    fn a_closure_of_a_superseded_version_runs_after_a_collection() {
        let (mut interpreter, outer) = load(OUTER);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let block = send(&mut interpreter, &instance, "make");
        drop(send_with(&mut interpreter, &instance, "kept:", &[block]));
        let id = id_of(&mut interpreter, &outer);
        for answer in 8..=10 {
            install(
                &mut interpreter,
                id,
                &add_method(&format!("public make = ( ^[ {answer} ] )")),
            );
        }
        interpreter.collect_now();
        assert!(table_counts(&mut interpreter).1 <= 1);
        let kept = send(&mut interpreter, &instance, "kept");
        let answer = send(&mut interpreter, &kept, "value");
        assert_eq!(integer(&mut interpreter, &answer), Some(7));
        let fresh = send(&mut interpreter, &instance, "make");
        let answer = send(&mut interpreter, &fresh, "value");
        assert_eq!(integer(&mut interpreter, &answer), Some(10));
    }

    /// A hundred closures, each of a body of its own -- the method is
    /// edited every round -- and each dropped: the collection frees the
    /// code ids nothing names, so the code table ends no larger than after
    /// the first round.
    #[cfg(debug_assertions)]
    #[test]
    fn codes_nothing_names_are_freed() {
        let (mut interpreter, outer) = load(OUTER);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let id = id_of(&mut interpreter, &outer);
        let mut after_first_round = None;
        for round in 1..=100 {
            install(
                &mut interpreter,
                id,
                &add_method(&format!("public make = ( ^[ {round} ] )")),
            );
            let block = send(&mut interpreter, &instance, "make");
            let answer = send(&mut interpreter, &block, "value");
            assert_eq!(integer(&mut interpreter, &answer), Some(round));
            drop((block, answer));
            if round == 1 {
                interpreter.collect_now();
                after_first_round = Some(table_counts(&mut interpreter).2);
            }
        }
        interpreter.collect_now();
        let after_first_round = after_first_round.expect("the first round ran");
        assert!(table_counts(&mut interpreter).2 <= after_first_round);
    }

    /// Each round edits `count` anew, runs the new body compiled -- the
    /// stress setting compiles it at its first entry -- and collects, which
    /// lets the old body die and frees its code id for a later body. The
    /// zone keeps no body alive and records no id a collection can free:
    /// every installed body's return address answers that body, under the
    /// id the heap has for it now, or nothing once it died -- never another
    /// body, though ids are minted again.
    #[cfg(all(ns_jit, debug_assertions))]
    #[test]
    fn a_return_address_never_names_a_body_that_took_a_freed_id() {
        let (mut interpreter, outer) = load(OUTER);
        interpreter.set_jit_enabled(true);
        interpreter.set_jit_stress(true);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let id = id_of(&mut interpreter, &outer);
        // Every body each id was answered for, by its allocation.
        let mut bodies_by_id: std::collections::HashMap<crate::heap::CodeId, Vec<usize>> =
            std::collections::HashMap::new();
        let mut died = 0;
        for round in 1..=20 {
            install(
                &mut interpreter,
                id,
                &add_method(&format!(
                    "public count = ( | sum ::= 0. index ::= 0. | [index < {round}] whileTrue: [ sum:: sum + index. index:: index + 1 ]. ^sum + {round} )"
                )),
            );
            let answer = send(&mut interpreter, &instance, "count");
            assert_eq!(
                integer(&mut interpreter, &answer),
                Some(round * (round - 1) / 2 + round)
            );
            drop(answer);
            interpreter.collect_now();
            interpreter.enter(|mutator| {
                for (return_address, body) in mutator.jit.installed_bodies_for_test() {
                    let answered = mutator.jit.body_at(return_address, &mut mutator.heap);
                    match (body, answered) {
                        (Some(body), Some((code_id, _))) => {
                            let named = mutator.heap.code(code_id).expect("a live row");
                            assert!(Rc::ptr_eq(&named, &body), "its own body, no other");
                            let allocation = Rc::as_ptr(&body) as usize;
                            let seen = bodies_by_id.entry(code_id).or_default();
                            if !seen.contains(&allocation) {
                                seen.push(allocation);
                            }
                        }
                        (None, None) => died += 1,
                        (Some(_), None) => panic!("a live body's return address answers nothing"),
                        (None, Some(_)) => panic!("a dead body's return address answers a body"),
                    }
                }
            });
        }
        // The zone stress mode's compactions free a dead body's code, which
        // no installed body then names.
        assert!(
            died > 0 || crate::jit::stress_zone_from_env(),
            "edited-away bodies die with their mixin"
        );
        assert!(
            bodies_by_id.values().any(|bodies| bodies.len() > 1),
            "some id was minted again for another body"
        );
    }

    /// Every table a hundred edit-install-call-collect rounds could grow.
    #[derive(Debug, PartialEq, Eq)]
    struct TableSizes {
        mixin_rows: usize,
        superseded: usize,
        application_rows: usize,
        successors: usize,
        code_rows: usize,
        cached_applications: usize,
        metaclass_mixins: usize,
        cache_sites: usize,
        guard_high_water: u32,
    }

    fn table_sizes(interpreter: &mut Interpreter) -> TableSizes {
        interpreter.enter(|mutator| TableSizes {
            mixin_rows: mutator.heap.mixin_rows(),
            superseded: mutator.heap.superseded_count(),
            application_rows: mutator.heap.application_rows(),
            successors: mutator.heap.successor_count(),
            code_rows: mutator.heap.code_rows(),
            cached_applications: mutator.applications.len(),
            metaclass_mixins: mutator.metaclass_mixins.len(),
            cache_sites: mutator.caches.table_len(),
            guard_high_water: mutator.dispatch_ids.high_water(),
        })
    }

    impl TableSizes {
        /// Whether no table is larger than in `bound`.
        fn within(&self, bound: &TableSizes) -> bool {
            self.mixin_rows <= bound.mixin_rows
                && self.superseded <= bound.superseded
                && self.application_rows <= bound.application_rows
                && self.successors <= bound.successors
                && self.code_rows <= bound.code_rows
                && self.cached_applications <= bound.cached_applications
                && self.metaclass_mixins <= bound.metaclass_mixins
                && self.cache_sites <= bound.cache_sites
                && self.guard_high_water <= bound.guard_high_water
        }
    }

    /// A hundred rounds, each installing an edit of `value`, calling it and
    /// collecting, grow no table past the size the second round left it,
    /// and the hundredth ends at exactly that size: each round's dead
    /// version, body, family and ids are given back and taken again by the
    /// next. The second round, not the first, is the measure: from it on,
    /// each round's body is given its range while the previous round's
    /// still holds one -- the collection that frees it comes after the
    /// call -- so the family alternates between two ranges, the first
    /// round's having had no predecessor; and the first round's
    /// applications fill holes the boot's collection left, which the
    /// second round's give back from the table's end.
    #[cfg(debug_assertions)]
    #[test]
    fn a_hundred_edits_grow_no_table_past_its_second_round_size() {
        let (mut interpreter, outer) = load(OUTER);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let id = id_of(&mut interpreter, &outer);
        let mut after_second_round = None;
        for round in 1..=100 {
            install(
                &mut interpreter,
                id,
                &add_method(&format!("public value = ( ^{round} yourself )")),
            );
            let answer = send(&mut interpreter, &instance, "value");
            assert_eq!(integer(&mut interpreter, &answer), Some(round));
            drop(answer);
            interpreter.collect_now();
            let sizes = table_sizes(&mut interpreter);
            if round == 2 {
                after_second_round = Some(sizes);
            } else if let Some(bound) = &after_second_round {
                assert!(
                    sizes.within(bound),
                    "round {round}: {sizes:?} outgrew {bound:?}"
                );
            }
        }
        let after_second_round = after_second_round.expect("the second round ran");
        assert_eq!(table_sizes(&mut interpreter), after_second_round);
    }

    const KEEPER: &str = indoc! {"
        Newspeak3
        'Test'
        class Keeper = (
          | public kept ::= nil. |
        ) (
          public make = (
            ^[:object | object size]
          )
          public probe = (
            ^kept value: {1. 2. 3}
          )
        )
    "};

    crate::for_each_lowering! {
        /// A closure whose method an edit superseded keeps its family's
        /// range across a collection: another family run afterwards takes
        /// the smallest free range, and the closure's site, monomorphic on
        /// `Array`s, still answers what it filled. `drop_superseded`'s
        /// running-body rule already keeps the root body here, so this
        /// guards the superseded-but-running path;
        /// `a_family_nothing_runs_gives_its_range_back` is what guards
        /// `live_bases`.
        #[cfg(debug_assertions)]
        fn a_closure_keeps_its_feedback_range_across_a_collection(lowering) {
            let (mut interpreter, keeper) = load_with(lowering, KEEPER);
            let instance = expect_ok(interpreter.instantiate(&keeper, &[]), "a Keeper");
            let block = send(&mut interpreter, &instance, "make");
            drop(send_with(&mut interpreter, &instance, "kept:", &[block]));
            for _ in 0..3 {
                let answer = send(&mut interpreter, &instance, "probe");
                assert_eq!(integer(&mut interpreter, &answer), Some(3));
            }
            let id = id_of(&mut interpreter, &keeper);
            install(&mut interpreter, id, &add_method("public make = ( ^[:object | object hash] )"));
            interpreter.collect_now();
            install(
                &mut interpreter,
                id,
                &add_method("public other = ( ^{1. 2. 3. 4. 5} size yourself )"),
            );
            let answer = send(&mut interpreter, &instance, "other");
            assert_eq!(integer(&mut interpreter, &answer), Some(5));
            let answer = send(&mut interpreter, &instance, "probe");
            assert_eq!(integer(&mut interpreter, &answer), Some(3));
        }
    }

    crate::for_each_lowering! {
        /// A guard a collection freed is minted again for a new class, and
        /// no cache entry filled for the dead class answers for it: the
        /// monomorphic site, the polymorphic block and the global cache
        /// behind the megamorphic site all send the new class's `value`.
        /// The reference lowering, which caches nothing, is the oracle.
        #[cfg(debug_assertions)]
        fn a_reused_guard_misses_every_cache_entry_of_its_old_application(lowering) {
            let (mut interpreter, class) = load_with(lowering, GUARDS);
            let instance = expect_ok(interpreter.instantiate(&class, &[]), "a Guards");
            let instance_of_a = send(&mut interpreter, &instance, "newA");
            let guard = interpreter.enter(|mutator| mutator.heap.dispatch_id(mutator.fetch(&instance_of_a)));
            drop(instance_of_a);
            drop(send(&mut interpreter, &instance, "drive"));
            interpreter.collect_now();
            // The guards freed below `A`'s are minted first; `A`'s, once
            // minted, is given back so that the next application takes it.
            interpreter.enter(|mutator| {
                let high_water = mutator.dispatch_ids.high_water();
                loop {
                    let minted = mutator.dispatch_ids.mint().expect("a guard id");
                    if minted == guard {
                        mutator.dispatch_ids.free(minted);
                        break;
                    }
                    assert!(minted.bits() < high_water, "A's guard was not freed");
                }
            });
            let class_c = send(&mut interpreter, &instance, "C");
            let instance_of_c = send(&mut interpreter, &class_c, "new");
            let reused = interpreter.enter(|mutator| mutator.heap.dispatch_id(mutator.fetch(&instance_of_c)));
            assert!(reused == guard, "C's instances carry A's old guard");
            for selector in ["probe:", "poly:", "mega:"] {
                let answer = send_with(&mut interpreter, &instance, selector, std::slice::from_ref(&instance_of_c));
                assert_eq!(integer(&mut interpreter, &answer), Some(3), "{selector}");
            }
        }
    }
}
