//! Metaclasses (`newspeak-spec.pdf` §3.2, §6): every class object's
//! metaclass is its class side -- together with its primary factory,
//! §6.3 -- applied to `Class`, built together with the class object
//! (`docs/design/object-model.md`, "Class sides and metaclasses").

use std::rc::Rc;

use crate::Symbol;
use crate::types::*;

use super::Mutator;

impl<'gc> Mutator<'gc> {
    /// `class`'s metaclass -- its class, since §3.2 makes every class "an
    /// instance of a unique metaclass" -- which exists for every class
    /// object once the bootstrap has filled `Class`
    /// (`give_existing_class_objects_metaclasses`); `None` before that, for
    /// a metaclass, whose class is `Metaclass`, and for a handle that is no
    /// class object.
    ///
    /// `newspeak-spec.pdf` §3.2: "All metaclasses are instances of class
    /// Metaclass, and are direct subclasses of Class. Metaclasses may not be
    /// subclassed. [...] The metaclass hierarchy in Newspeak is deliberately
    /// kept very flat." §6 gives the construction: the class is an instance
    /// of "a metaclass that is the application of the metaclass mixin defined
    /// by the class' class side to the class Class". So a class side is not
    /// inherited: a class-side send that misses on the metaclass continues
    /// onto `Class`, never onto an ancestor's class side.
    ///
    /// Every class object has a metaclass, including one whose declaration
    /// writes no class side: §6.3 makes the metaclass mixin the class side
    /// *together with the primary factory method*, so `X new` is an entry of
    /// this mixin's method table and a class without one would not be
    /// instantiable. The empty case is `metaclass_mixin` below.
    pub fn metaclass_of(&self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>> {
        self.heap.class_metaclass(class)
    }

    /// Builds `class`'s metaclass: §6's "the application of the metaclass
    /// mixin defined by the class' class side to the class `Class`". Its
    /// class-object superclass is `Class`, so a class-side `super` send
    /// searches `Class` (§5.10), and it shares `class`'s place in the
    /// declaration chain, since §6.3 writes a class side inside the same
    /// declaration as its instance side.
    ///
    /// The application is the memoized one for (class side, `Class`), so
    /// every class object of one declaration -- one per enclosing instance
    /// -- gets a metaclass of one application, and so one dispatch id: a
    /// per-class-object application would send every `X new` site
    /// megamorphic.
    ///
    /// Both headers change here (§3.2): the class object becomes an
    /// instance of its metaclass, and the metaclass an instance of
    /// `Metaclass`. So the class object's dispatch id is its metaclass's
    /// instances id from here on, and a send to it searches its class side,
    /// then `Class`, `Behavior` and `Object`, by the ordinary lookup.
    pub(crate) fn attach_metaclass(&mut self, class: ClassReference<'gc>) -> ClassReference<'gc> {
        let application = self
            .heap
            .class_application(class)
            .expect("internal: attach_metaclass is handed a class object");
        let class_side_mixin = self.metaclass_mixin(&application.mixin);
        self.attach_metaclass_of(class, class_side_mixin)
    }

    /// `attach_metaclass` with the class side the metaclass applies given,
    /// rather than read off `class`'s own mixin: a level a `<:` clause
    /// added takes the class side of the class its clause named, and a
    /// body-less declaration's class object its declaration's
    /// (`Mutator::new_applied_level`), so that each has the factory and
    /// the name §6.2 gives it (`object-model.md`, "Mixin application").
    pub(crate) fn attach_metaclass_of(
        &mut self,
        class: ClassReference<'gc>,
        class_side_mixin: Rc<crate::mixin::Mixin>,
    ) -> ClassReference<'gc> {
        // A metaclass's class is `Metaclass`, never a metaclass of its own:
        // `object_set_class(class, metaclass)` below on a metaclass would
        // overwrite its `Metaclass` header, breaking §3.2's "every
        // metaclass is an instance of Metaclass" and the image decoder's
        // pairing.
        debug_assert!(
            self.heap.class_this_class(class).is_none(),
            "internal: a metaclass never gets a metaclass of its own"
        );
        let class_class = self.builtin_class(BuiltinClass::Class);
        let class_application = self.heap.class_application(class_class);
        let metaclass_application = self.apply_mixin(&class_side_mixin, class_application.as_ref());
        let data = crate::heap::NewClass {
            application: metaclass_application,
            enclosing: self.heap.class_enclosing(class),
            declaring_class: self.heap.class_declaring(class),
            superclass: Some(class_class),
            builtin: None,
        };
        let metaclass = self.allocate_class_object(data);
        if !self.heap.class_set_this_class(metaclass, class) {
            unreachable!("internal: a metaclass is a class object");
        }
        let metaclass_class = self.builtin_class(BuiltinClass::Metaclass);
        if !self.heap.object_set_class(metaclass, metaclass_class)
            || !self.heap.object_set_class(class, metaclass)
        {
            unreachable!("internal: `Metaclass` and a metaclass are class objects");
        }
        metaclass
    }

    /// The mixin `attach_metaclass` applies for `mixin`'s class objects:
    /// §6.3's "the class side, together with the primary factory method".
    ///
    /// A declared class side already carries the factory entry -- it is
    /// inserted where the class side is built, from source
    /// (`instantiate::build_class_side`) and on the way out of an image
    /// (`image::mixins::build`) alike. A class that declares none still
    /// needs a metaclass to hold that one entry, and gets an otherwise empty
    /// mixin here: the case that covers every builtin, whose behaviour is
    /// native and whose factory is `new` -- the default §6's unnumbered
    /// introduction gives an omitted factory signature, before §6.1 begins.
    pub(crate) fn metaclass_mixin(
        &mut self,
        mixin: &Rc<crate::mixin::Mixin>,
    ) -> Rc<crate::mixin::Mixin> {
        if let Some(class_side) = &mixin.class_side {
            return class_side.clone();
        }
        // Keyed by the instance side's id, which interns it -- the decoder
        // asks for the class side of an image's row before anything else
        // interns the row. A collection that frees the id's tree drops
        // the entry with it, so no key outlives its mixin.
        let key = self.heap.mixin_id(mixin);
        if let Some(existing) = self.metaclass_mixins.get(&key) {
            return existing.clone();
        }
        let synthesized = synthesized_class_side(mixin);
        self.metaclass_mixins.insert(key, synthesized.clone());
        synthesized
    }

    /// The class side `class`'s metaclass applies -- the one that gives it
    /// its factory and its name -- or, for a class object without a
    /// metaclass yet, the one `metaclass_mixin` gives `mixin`, its own
    /// mixin.
    pub(crate) fn class_side_of(
        &mut self,
        class: ClassReference<'gc>,
        mixin: &Rc<crate::mixin::Mixin>,
    ) -> Rc<crate::mixin::Mixin> {
        match self
            .heap
            .class_metaclass(class)
            .and_then(|metaclass| self.heap.class_application(metaclass))
        {
            Some(class_side) => class_side.mixin.clone(),
            None => self.metaclass_mixin(mixin),
        }
    }

    /// The end of the kernel phase: every class object built before
    /// `Class` was filled -- the builtins and `others`, the kernel's
    /// `Value` level and the kernel's own class -- gets its metaclass, and
    /// from here on `new_class_object` builds one with every class object.
    ///
    /// Until here every one of them had the builtin `Object` as its
    /// provisional header, and `Object` had itself; each header becomes
    /// its metaclass now, `Object`'s and `Metaclass`'s included --
    /// `Metaclass`'s metaclass is an instance of `Metaclass` like every
    /// other metaclass.
    pub(crate) fn give_existing_class_objects_metaclasses(
        &mut self,
        others: &[ClassReference<'gc>],
    ) {
        let builtins: Vec<ClassReference<'gc>> = BuiltinClass::ALL
            .into_iter()
            .map(|kind| self.builtin_class(kind))
            .collect();
        for class in builtins.into_iter().chain(others.iter().copied()) {
            if self.heap.class_metaclass(class).is_none() {
                self.attach_metaclass(class);
            }
        }
        self.metaclasses_ready = true;
    }
}

/// The class side `metaclass_mixin` synthesizes for `mixin`, which
/// declares none: the factory entry alone, linked to `mixin` as a declared
/// class side is. Uncached, so that an edit can build the one its fresh
/// instance side needs without registering or recording anything.
pub(crate) fn synthesized_class_side(mixin: &Rc<crate::mixin::Mixin>) -> Rc<crate::mixin::Mixin> {
    let synthesized = Rc::new(crate::mixin::Mixin {
        methods: std::iter::once(crate::mixin::factory_entry(mixin.factory)).collect(),
        text: mixin.text.clone(),
        ..crate::mixin::Mixin::empty(Symbol::CLASS_SIDE)
    });
    // A synthesized class side names the instance side it stands for and
    // shares its text, as a declared one does. The cell is fresh, so the
    // set cannot fail.
    let _ = synthesized.enclosing.set(Rc::downgrade(mixin));
    synthesized
}

/// The class side `metaclass_mixin` synthesized for each instance side
/// that declares none, keyed by the instance side's id
/// (`Mutator::metaclass_mixins`), with a generation raised by every insert
/// and removal. Reads go through the map; changes only through this, so
/// that none can leave a full collection's reclamation counting a
/// version's holders by a memo the map no longer matches
/// (`interpreter::reclaim`, `MixinReclaimMemo`).
#[derive(Default)]
pub(crate) struct MetaclassMixins {
    map: crate::hash::IdentityMap<crate::heap::MixinId, Rc<crate::mixin::Mixin>>,
    generation: u64,
}

impl MetaclassMixins {
    /// Records `synthesized` for the instance side `id`.
    pub(crate) fn insert(
        &mut self,
        id: crate::heap::MixinId,
        synthesized: Rc<crate::mixin::Mixin>,
    ) {
        self.map.insert(id, synthesized);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Forgets the instance side `id`'s synthesized class side, if any.
    pub(crate) fn remove(&mut self, id: crate::heap::MixinId) {
        if self.map.remove(&id).is_some() {
            self.generation = self.generation.wrapping_add(1);
        }
    }

    /// Raised by every insert and every removal that removed something.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

impl std::ops::Deref for MetaclassMixins {
    type Target = crate::hash::IdentityMap<crate::heap::MixinId, Rc<crate::mixin::Mixin>>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::Signal;
    use crate::{HostResult, HostSignal, Interpreter, Root, SourceOrigin};
    use indoc::indoc;
    use std::path::PathBuf;

    fn load(source: &str) -> (Interpreter, Root) {
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(source),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
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

    /// Every class object has a metaclass whose `this_class` is it, and
    /// every builtin's metaclass has its class's lexical context -- the
    /// one the kernel phase gave the builtin after the metaclass was built.
    #[test]
    fn every_builtin_has_a_metaclass_in_its_declaration_context() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            for kind in BuiltinClass::ALL {
                let class = mutator.builtin_class(kind);
                // Read off the class object itself: the bootstrap attaches
                // every builtin's metaclass, so one it skipped fails here.
                let heap = mutator.heap();
                let metaclass = heap
                    .class_metaclass(class)
                    .unwrap_or_else(|| panic!("{kind:?} has a metaclass"));
                assert!(
                    heap.class_this_class(metaclass)
                        .is_some_and(|this_class| this_class.same(class)),
                    "{kind:?}'s metaclass links back to it"
                );
                assert!(
                    heap.class_this_class(class).is_none(),
                    "{kind:?} is no metaclass"
                );
                let same_enclosing =
                    match (heap.class_enclosing(metaclass), heap.class_enclosing(class)) {
                        (Some(metaclass_enclosing), Some(class_enclosing)) => {
                            metaclass_enclosing.same(class_enclosing)
                        }
                        (None, None) => true,
                        _ => false,
                    };
                assert!(
                    same_enclosing,
                    "{kind:?}'s metaclass has its enclosing object"
                );
                let same_declaring =
                    match (heap.class_declaring(metaclass), heap.class_declaring(class)) {
                        (Some(metaclass_declaring), Some(class_declaring)) => {
                            metaclass_declaring.same(class_declaring)
                        }
                        (None, None) => true,
                        _ => false,
                    };
                assert!(
                    same_declaring,
                    "{kind:?}'s metaclass has its declaring class"
                );
                if kind != BuiltinClass::VmMirror {
                    assert!(
                        heap.class_enclosing(metaclass).is_some(),
                        "{kind:?}'s metaclass is enclosed by the kernel instance"
                    );
                }
            }
        });
    }

    /// The class side synthesized for a mixin nothing has applied -- an
    /// image's row, as the decoder's first pass asks for it -- is keyed by
    /// an id that keeps the mixin alive, so the key cannot outlive it and
    /// answer its class side for another mixin later at the same address.
    #[test]
    fn a_synthesized_class_side_keeps_its_instance_side_alive() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let mixin = Rc::new(crate::mixin::Mixin::empty(Symbol::intern("Unapplied")));
            let first = mutator.metaclass_mixin(&mixin);
            let weak = Rc::downgrade(&mixin);
            drop(mixin);
            let Some(instance_side) = weak.upgrade() else {
                panic!("the mixin a class side is keyed by was dropped");
            };
            let second = mutator.metaclass_mixin(&instance_side);
            assert!(Rc::ptr_eq(&first, &second), "one class side per mixin");
        });
    }

    /// §3.2: nothing below `Behavior` can be instantiated.
    #[test]
    fn a_metaclass_and_the_three_kernel_classes_refuse_instantiation() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let object = mutator.builtin_class(BuiltinClass::Object);
            let object_metaclass = mutator
                .metaclass_of(object)
                .expect("Object has a metaclass");
            let refused = [
                (mutator.builtin_class(BuiltinClass::Behavior), "Behavior"),
                (mutator.builtin_class(BuiltinClass::Class), "Class"),
                (mutator.builtin_class(BuiltinClass::Metaclass), "Metaclass"),
                (object_metaclass, "Object class"),
            ];
            for (class, name) in refused {
                let message = match mutator.allocate_for_factory(class) {
                    Err(Signal::Error(error)) => String::from(error.message),
                    Err(_) => panic!("{name}: refused with something other than an error"),
                    Ok(_) => panic!("{name}: instantiated"),
                };
                assert_eq!(
                    message,
                    format!(
                        "cannot instantiate {name}: a class object is made only by evaluating its declaration"
                    )
                );
            }
        });
    }

    /// Two class objects of one declaration under two enclosing instances
    /// have metaclasses of one application.
    #[test]
    fn metaclasses_of_one_declaration_share_an_application() {
        let source = indoc! {"
            class Outer usingPlatform: platform = Object new () (
                public class Widget = Object new () (
                )
            )
        "};
        let (mut interpreter, class) = load(source);
        let platform = interpreter.platform();
        let first_outer = expect_ok(
            interpreter.instantiate(&class, std::slice::from_ref(&platform)),
            "instantiate the first Outer",
        );
        let second_outer = expect_ok(
            interpreter.instantiate(&class, &[platform]),
            "instantiate the second Outer",
        );
        let widget = Symbol::intern("Widget");
        let first_widget = expect_ok(interpreter.send(&first_outer, widget, &[]), "Widget");
        let second_widget = expect_ok(interpreter.send(&second_outer, widget, &[]), "Widget");
        interpreter.enter(|mutator| {
            let (Some(first_widget), Some(second_widget)) = (
                mutator.fetch(&first_widget).as_object(),
                mutator.fetch(&second_widget).as_object(),
            ) else {
                panic!("Widget answers a class object");
            };
            assert!(
                !first_widget.same(second_widget),
                "one class object per enclosing instance"
            );
            let first_metaclass = mutator
                .metaclass_of(first_widget)
                .expect("every class object has a metaclass");
            let second_metaclass = mutator
                .metaclass_of(second_widget)
                .expect("every class object has a metaclass");
            assert!(!first_metaclass.same(second_metaclass));
            let heap = mutator.heap();
            let first_application = heap
                .class_application(first_metaclass)
                .expect("a metaclass is a class object");
            let second_application = heap
                .class_application(second_metaclass)
                .expect("a metaclass is a class object");
            assert!(Rc::ptr_eq(&first_application, &second_application));
        });
    }
}
