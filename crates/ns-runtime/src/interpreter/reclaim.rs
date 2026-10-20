//! A full collection's reclamation of the ids nothing names any more
//! (`Mutator::reclaim_ids`; `docs/design/object-model.md`, "An application
//! has two ids").

use std::collections::HashSet;
use std::rc::Rc;

use super::Mutator;
use crate::hash::{IdentityBuildHasher, IdentityMap};
use crate::heap::reclaim::{FreedIds, GatheredIds};
use crate::heap::ApplicationIndex;
use crate::mixin::DispatchId;

impl Mutator<'_> {
    /// Frees every application nothing names any more, with its index and
    /// its guard id, and vacates exactly the cache entries that named one,
    /// so that the next application minted at a freed index or with a
    /// freed guard is never mistaken for the dead one. Runs in a full
    /// collection between marking and compaction (`World::collect`):
    /// `gathered` is what the marked objects name, and nothing here moves
    /// or allocates an object.
    ///
    /// The steps run in order, each releasing the `Rc`s the next one
    /// counts: the applications, then the caches that hold them.
    pub(crate) fn reclaim_ids(&mut self, gathered: GatheredIds) {
        let stack = self.heap.stack_roots();
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
    use crate::interpreter::{HostResult, HostSignal, Interpreter};
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
    }

    const SUPERSEDED: &str = indoc! {"
        Newspeak3
        'Test'
        class Superseded = (
        ) (
          class Subject during: action = (
            |
              public first = action value.
              public second = 2.
            |
          ) (
          )
          public run: platform = (
            | builder instance |
            builder:: (platform mirrors ClassMirror reflecting: Subject) mixin declaration
                asBuilder.
            builder instanceSide methods addFromSource: 'public added = (^3)'.
            instance:: Subject during: [
              builder install.
              platform kernel garbageCollect.
              1
            ].
            ^instance first + instance second + instance added
          )
        )
    "};

    /// An initializer whose class an install re-applied, keeping its
    /// slots, runs on at the level it was entered at across the full
    /// collection that follows: the superseded application stays, so the
    /// slot after the install is written where the code expects it.
    #[test]
    fn an_activation_at_a_superseded_level_runs_on_across_a_collection() {
        let (mut interpreter, class) = load(SUPERSEDED);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "a Superseded");
        let platform = interpreter.platform();
        let collections = interpreter.collector_statistics().collections;
        let answer = send_with(&mut interpreter, &instance, "run:", &[platform]);
        assert!(interpreter.collector_statistics().collections > collections);
        assert_eq!(integer(&mut interpreter, &answer), Some(6));
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

    crate::for_each_lowering! {
        /// A guard a collection freed is minted again for a new class, and
        /// no cache entry filled for the dead class answers for it: the
        /// monomorphic site, the polymorphic block and the global cache
        /// behind the megamorphic site all send the new class's `value`.
        /// The reference lowering, which caches nothing, is the oracle.
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
