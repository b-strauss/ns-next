//! Metaclasses (`newspeak-spec.pdf` §3.2, §6): every class object's
//! metaclass is its class side -- together with its primary factory,
//! §6.3 -- applied to `Class`, built together with the class object
//! (`docs/design/object-model.md`, "Class sides and metaclasses").

use std::rc::Rc;

use crate::types::*;
use crate::Symbol;

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
        // A metaclass's class is `Metaclass`, never a metaclass of its own:
        // `object_set_class(class, metaclass)` below on a metaclass would
        // overwrite its `Metaclass` header, breaking §3.2's "every
        // metaclass is an instance of Metaclass" and the image decoder's
        // pairing.
        debug_assert!(
            self.heap.class_this_class(class).is_none(),
            "internal: a metaclass never gets a metaclass of its own"
        );
        let application = self
            .heap
            .class_application(class)
            .expect("internal: attach_metaclass is handed a class object");
        let class_side_mixin = self.metaclass_mixin(&application.mixin);
        let class_class = self.builtin_class(BuiltinClass::Class);
        let class_application = self.heap.class_application(class_class);
        let metaclass_application = self.apply_mixin(&class_side_mixin, class_application.as_ref());
        let data = ClassData::new(
            metaclass_application,
            self.heap.class_enclosing(class),
            self.heap.class_declaring(class),
            Some(class_class),
            None,
        );
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
    /// (`image::mixins::decode`) alike. A class that declares none still
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
        let key = crate::mixin::mixin_key(mixin);
        if let Some(existing) = self.metaclass_mixins.get(&key) {
            return existing.clone();
        }
        let synthesized = Rc::new(crate::mixin::Mixin {
            methods: std::iter::once(crate::mixin::factory_entry(mixin.factory)).collect(),
            ..crate::mixin::Mixin::empty(Symbol::CLASS_SIDE)
        });
        self.metaclass_mixins.insert(key, synthesized.clone());
        synthesized
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
                let message = match mutator.decide_instantiation(class, Vec::new()) {
                    Err(Signal::Error(error)) => error.message,
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
