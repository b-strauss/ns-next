//! A trace event's bodies named as the debugger names a frame
//! (`debugger/stack.rs`'s `frame_name`): the owner qualified from its
//! module down (`qualified_name`, `docs/design/modules.md`, decision 8),
//! then `>>` and the selector, ` initializer` or ` superclass clause`, a
//! block `[] in ` and the name of the body it is created in.
//!
//! A pending event holds a body as its code alone, and code knows neither
//! its mixin nor its member, so a body is found by an index over every
//! body of every mixin the heap holds, built afresh when a body is not in
//! it and cached between flushes.

use std::collections::HashMap;
use std::rc::{Rc, Weak};

use crate::bytecode::CompiledCode;
use crate::interpreter::Mutator;
use crate::interpreter::instantiate::InitializerBody;
use crate::mixin::{MethodEntry, Mixin};

/// The name of a body no mixin of the heap holds.
const UNNAMED: &str = "(unnamed)";

/// The name of every body found so far, keyed by its address.
#[derive(Default)]
pub(crate) struct BodyNames {
    /// Each body's address, the body itself held weakly -- so that the
    /// index keeps no body alive and an address a dropped body's
    /// successor reuses is never answered the dropped one's name -- and
    /// its name.
    names: HashMap<usize, (Weak<CompiledCode>, String)>,
}

impl BodyNames {
    /// The name of `code`: cached, or found by building the index afresh
    /// over every mixin of the heap, `(unnamed)` where none holds it.
    pub(crate) fn name(&mut self, mutator: &Mutator<'_>, code: &Rc<CompiledCode>) -> String {
        if let Some(name) = self.cached(code) {
            return name;
        }
        self.rebuild(mutator);
        if let Some(name) = self.cached(code) {
            return name;
        }
        // A body no mixin holds -- a factory body a host's instantiation
        // built for itself -- comes to be held by none later, so its name
        // is kept rather than looked for at every event.
        self.names
            .insert(address(code), (Rc::downgrade(code), UNNAMED.to_string()));
        UNNAMED.to_string()
    }

    /// The name cached for `code`, if the entry at its address is its own.
    fn cached(&self, code: &Rc<CompiledCode>) -> Option<String> {
        let (body, name) = self.names.get(&address(code))?;
        let alive = body.upgrade()?;
        Rc::ptr_eq(&alive, code).then(|| name.clone())
    }

    /// The index built afresh: every body of every mixin the heap holds,
    /// current versions and those an edit superseded, both sides -- a
    /// synthesized class side among the mixins by its own id, and its
    /// superseded versions among the superseded ones. A body found
    /// `(unnamed)` stays so while it lives: no mixin comes to hold a body
    /// that exists already, so looking for it again would only walk every
    /// mixin once more.
    fn rebuild(&mut self, mutator: &Mutator<'_>) {
        self.names
            .retain(|_, (body, name)| name == UNNAMED && body.strong_count() > 0);
        let heap = &mutator.heap;
        let mut sides: Vec<Rc<Mixin>> = Vec::new();
        let mut add_level = |mixin: Rc<Mixin>, synthesized: Option<Rc<Mixin>>| {
            if let Some(class_side) = mixin.class_side.clone().or(synthesized) {
                sides.push(class_side);
            }
            sides.push(mixin);
        };
        for id in heap.mixin_ids() {
            if let Some(mixin) = heap.mixin(id) {
                let synthesized = mutator.metaclass_mixins.get(&id).cloned();
                add_level(mixin, synthesized);
            }
        }
        for mixin in heap.superseded_mixins() {
            add_level(Rc::clone(mixin), None);
        }
        for side in &sides {
            self.name_side(mutator, side);
        }
    }

    /// Names every body of `side` -- its methods, its factory, its
    /// initializer bodies, its superclass clause -- and the blocks each
    /// creates, transitively, after it. A body already named keeps its
    /// name: the current version's, named first.
    fn name_side(&mut self, mutator: &Mutator<'_>, side: &Rc<Mixin>) {
        let owner = crate::debugger::metadata::again::qualified_name(mutator, side)
            .unwrap_or_else(|| side.reflected_name());
        let mut roots: Vec<(Rc<CompiledCode>, String)> = Vec::new();
        for (selector, entry) in &side.methods {
            match entry {
                MethodEntry::Compiled(method) => {
                    roots.push((Rc::clone(&method.code), format!("{owner}>>{selector}")));
                }
                MethodEntry::Factory(factory) => {
                    roots.push((Rc::clone(&factory.code), format!("{owner}>>{selector}")));
                }
                MethodEntry::SlotAccessor(_) | MethodEntry::Primitive(_) => {}
            }
        }
        for kind in [
            InitializerBody::Full,
            InitializerBody::OwnPart,
            InitializerBody::Standalone,
        ] {
            if let Some(code) = kind.code_of(side) {
                roots.push((code, format!("{owner} initializer")));
            }
        }
        if let Some(code) = &side.superclass_clause_code {
            roots.push((Rc::clone(code), format!("{owner} superclass clause")));
        }
        for (root, name) in roots {
            let block_name = format!("[] in {name}");
            self.insert(&root, name);
            // A worklist: an image's body chooses how deep its blocks nest.
            let mut pending: Vec<Rc<CompiledCode>> = vec![root];
            while let Some(code) = pending.pop() {
                for block in code.body().created_blocks() {
                    if self.insert(block.code, block_name.clone()) {
                        pending.push(Rc::clone(block.code));
                    }
                }
            }
        }
    }

    /// Names `code` `name` unless it is named already; answers whether it
    /// was not.
    fn insert(&mut self, code: &Rc<CompiledCode>, name: String) -> bool {
        if self.cached(code).is_some() {
            return false;
        }
        self.names
            .insert(address(code), (Rc::downgrade(code), name));
        true
    }
}

/// The address `code` is keyed by.
fn address(code: &Rc<CompiledCode>) -> usize {
    Rc::as_ptr(code) as usize
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use indoc::indoc;

    use super::*;
    use crate::heap::MixinId;
    use crate::interpreter::edit::{Edit, MemberEdit};
    use crate::interpreter::{HostResult, HostSignal, Interpreter};
    use crate::types::SourceOrigin;
    use crate::{Root, Symbol};

    /// A module with a nested class, its factory, a class-side method, a
    /// slot initializer and a block.
    const DEMO: &str = indoc! {"
        class Demo = Object new () (
          public class Account named: name = Object new (
            |
              public name = name.
              public balance ::= 0.
            |
          ) (
            public deposit: amount = (
              balance:: balance + amount.
            )
          ) : (
            public opening = (
              ^self named: 'opening'.
            )
          )

          public class Plain = Object new (
            |
              public count ::= 1.
            |
          ) ()

          public main: platform args: arguments = (
            ^[ arguments ].
          )
        )
    "};

    /// A versioned module with a nested class.
    const UTIL: &str = indoc! {"
        (* :module: com.acme.util.Util@1.0.0 *)
        class Util = Object new () (
          public class Inner = Object new () (
            public run = (
              ^1.
            )
          )
        )
    "};

    fn expect_ok(result: HostResult, what: &str) -> Root {
        match result {
            Ok(root) => root,
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    /// `source` loaded, with the identity `identity` gives it, and its
    /// nested `Inner` class reached once, so that its mixin is registered.
    fn load(source: &str, identity: Option<(&str, &str)>, inner: &str) -> (Interpreter, Root) {
        load_reaching(source, identity, &[inner])
    }

    /// `source` loaded, with the identity `identity` gives it, and each
    /// of its nested classes `inner` names reached once.
    fn load_reaching(
        source: &str,
        identity: Option<(&str, &str)>,
        inner: &[&str],
    ) -> (Interpreter, Root) {
        let module = ns_syntax::parse(source).expect("parse");
        let mut resolved = crate::resolve(
            module,
            Rc::from(source),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        if let Some((id, version)) = identity {
            let class = id.rsplit('.').next().expect("a class");
            resolved.identity = Some(Rc::new(crate::modules::ModuleIdentity {
                path: "t.ns".to_string(),
                id: crate::modules::ModuleId::parse(id, class).expect("an id"),
                version: Some(crate::modules::Version::parse(version).expect("a version")),
            }));
        }
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "an instance");
        for inner in inner {
            expect_ok(
                interpreter.send(&instance, Symbol::intern(inner), &[]),
                inner,
            );
        }
        (interpreter, class)
    }

    /// The root mixin of the module whose class `class` is.
    fn root_mixin(mutator: &Mutator<'_>, class: &Root) -> Rc<Mixin> {
        let class = mutator.fetch(class).as_object().expect("a class object");
        Rc::clone(
            &mutator
                .heap
                .class_application(class)
                .expect("a class object")
                .mixin,
        )
    }

    /// The mixin of the declaration `name` nested in `mixin`.
    fn nested(mutator: &Mutator<'_>, mixin: &Rc<Mixin>, name: &str) -> Rc<Mixin> {
        mutator
            .heap
            .nested_mixins(mixin)
            .into_iter()
            .find(|nested| nested.name.as_str() == name)
            .expect("the declaration is nested there")
    }

    /// The class side of `mixin`, declared or synthesized.
    fn class_side(mutator: &Mutator<'_>, mixin: &Rc<Mixin>) -> Rc<Mixin> {
        mixin.class_side.clone().unwrap_or_else(|| {
            let id = mutator.heap.known_mixin_id(mixin).expect("registered");
            Rc::clone(mutator.metaclass_mixins.get(&id).expect("a class side"))
        })
    }

    /// The body of `mixin`'s method `selector`.
    fn method(mixin: &Mixin, selector: &str) -> Rc<CompiledCode> {
        Rc::clone(
            &mixin
                .compiled_method(Symbol::intern(selector))
                .expect("the method")
                .code,
        )
    }

    /// The body of `class_side`'s factory.
    fn factory(class_side: &Mixin) -> Rc<CompiledCode> {
        class_side
            .methods
            .values()
            .find_map(|entry| match entry {
                MethodEntry::Factory(factory) => Some(Rc::clone(&factory.code)),
                _ => None,
            })
            .expect("a factory")
    }

    /// The name `BodyNames` gives `code`, from an empty cache.
    fn name_of(mutator: &Mutator<'_>, code: &Rc<CompiledCode>) -> String {
        BodyNames::default().name(mutator, code)
    }

    #[test]
    fn a_method_is_named_from_its_module_down() {
        let (mut interpreter, class) = load(DEMO, None, "Account");
        interpreter.enter(|mutator| {
            let demo = root_mixin(mutator, &class);
            let account = nested(mutator, &demo, "Account");
            assert_eq!(
                name_of(mutator, &method(&account, "deposit:")),
                "Demo/Account>>deposit:"
            );
            assert_eq!(
                name_of(mutator, &method(&demo, "main:args:")),
                "Demo>>main:args:"
            );
        });
        let (mut interpreter, class) = load(UTIL, Some(("com.acme.util.Util", "1.0.0")), "Inner");
        interpreter.enter(|mutator| {
            let util = root_mixin(mutator, &class);
            let inner = nested(mutator, &util, "Inner");
            assert_eq!(
                name_of(mutator, &method(&inner, "run")),
                "com.acme.util.Util@1.0.0/Inner>>run"
            );
        });
    }

    #[test]
    fn a_class_side_method_and_a_factory() {
        let (mut interpreter, class) = load(DEMO, None, "Account");
        interpreter.enter(|mutator| {
            let demo = root_mixin(mutator, &class);
            let account = nested(mutator, &demo, "Account");
            let class_side = class_side(mutator, &account);
            assert_eq!(
                name_of(mutator, &factory(&class_side)),
                "Demo/Account class>>named:"
            );
            assert_eq!(
                name_of(mutator, &method(&class_side, "opening")),
                "Demo/Account class>>opening"
            );
        });
    }

    #[test]
    fn a_block_is_named_after_its_root() {
        let (mut interpreter, class) = load(DEMO, None, "Account");
        interpreter.enter(|mutator| {
            let demo = root_mixin(mutator, &class);
            let main = method(&demo, "main:args:");
            let block = Rc::clone(main.body().created_blocks().next().expect("the block").code);
            assert_eq!(name_of(mutator, &block), "[] in Demo>>main:args:");
        });
    }

    #[test]
    fn an_initializer_and_a_superclass_clause() {
        let (mut interpreter, class) = load(DEMO, None, "Account");
        interpreter.enter(|mutator| {
            let demo = root_mixin(mutator, &class);
            let account = nested(mutator, &demo, "Account");
            let initializer = account.initializer_code.clone().expect("an initializer");
            assert_eq!(name_of(mutator, &initializer), "Demo/Account initializer");
            let clause = account
                .superclass_clause_code
                .clone()
                .expect("a superclass clause");
            assert_eq!(name_of(mutator, &clause), "Demo/Account superclass clause");
        });
    }

    #[test]
    fn a_superseded_body_keeps_its_name() {
        let (mut interpreter, class) = load(DEMO, None, "Account");
        let (old, id): (Rc<CompiledCode>, MixinId) = interpreter.enter(|mutator| {
            let demo = root_mixin(mutator, &class);
            let account = nested(mutator, &demo, "Account");
            let id = mutator.heap.known_mixin_id(&account).expect("registered");
            (method(&account, "deposit:"), id)
        });
        let edit = Edit {
            header: None,
            instance_side: vec![MemberEdit::AddMethod(
                "public deposit: amount = ( balance:: balance + amount + 1 )".to_string(),
            )],
            class_side: Vec::new(),
        };
        if let Err(diagnostic) = interpreter.enter(|mutator| mutator.install_edit(id, &edit)) {
            panic!("the install was refused: {diagnostic}");
        }
        interpreter.enter(|mutator| {
            let current = mutator.heap.mixin(id).expect("the current version");
            let new = method(&current, "deposit:");
            assert!(!Rc::ptr_eq(&old, &new), "the edit compiled a new body");
            let mut names = BodyNames::default();
            assert_eq!(names.name(mutator, &new), "Demo/Account>>deposit:");
            assert_eq!(names.name(mutator, &old), "Demo/Account>>deposit:");
        });
    }

    /// A class that declares no class side has one synthesized, which an
    /// edit of its header replaces: the factory body of the version it
    /// replaced still runs in the frames that hold it, and keeps its name.
    #[test]
    fn a_superseded_synthesized_factory_keeps_its_name() {
        let (mut interpreter, class) = load_reaching(DEMO, None, &["Account", "Plain"]);
        let (old, id): (Rc<CompiledCode>, MixinId) = interpreter.enter(|mutator| {
            let demo = root_mixin(mutator, &class);
            let plain = nested(mutator, &demo, "Plain");
            assert!(plain.class_side.is_none(), "Plain declares no class side");
            let id = mutator.heap.known_mixin_id(&plain).expect("registered");
            (factory(&class_side(mutator, &plain)), id)
        });
        let edit = Edit {
            header: Some("public class Plain = Object new (| public count ::= 2. |)".to_string()),
            instance_side: Vec::new(),
            class_side: Vec::new(),
        };
        if let Err(diagnostic) = interpreter.enter(|mutator| mutator.install_edit(id, &edit)) {
            panic!("the install was refused: {diagnostic}");
        }
        interpreter.enter(|mutator| {
            let current = mutator.heap.mixin(id).expect("the current version");
            let new = factory(&class_side(mutator, &current));
            assert!(
                !Rc::ptr_eq(&old, &new),
                "the edit synthesized a new class side"
            );
            let mut names = BodyNames::default();
            assert_eq!(names.name(mutator, &new), "Demo/Plain class>>new");
            assert_eq!(names.name(mutator, &old), "Demo/Plain class>>new");
        });
    }

    /// A body found unnamed is not looked for again when another body's
    /// miss builds the index afresh.
    #[test]
    fn an_unnamed_body_survives_a_rebuild() {
        let (mut interpreter, class) = load(DEMO, None, "Account");
        interpreter.enter(|mutator| {
            let loose = Rc::new(crate::compiler::factory_body(3));
            let mut names = BodyNames::default();
            assert_eq!(names.name(mutator, &loose), "(unnamed)");
            names.rebuild(mutator);
            assert_eq!(
                names
                    .names
                    .get(&address(&loose))
                    .map(|(_, name)| name.as_str()),
                Some("(unnamed)")
            );
            let demo = root_mixin(mutator, &class);
            assert_eq!(
                names.name(mutator, &method(&demo, "main:args:")),
                "Demo>>main:args:"
            );
        });
    }

    #[test]
    fn a_name_cached_for_a_dropped_body_is_not_reused() {
        let (mut interpreter, class) = load(DEMO, None, "Account");
        interpreter.enter(|mutator| {
            let demo = root_mixin(mutator, &class);
            let account = nested(mutator, &demo, "Account");
            let deposit = method(&account, "deposit:");
            let mut names = BodyNames::default();
            // An entry at the body's address whose body is gone, as one a
            // body dropped before this one took its allocation leaves.
            let dropped = Rc::new(crate::compiler::factory_body(0));
            let stale = Rc::downgrade(&dropped);
            drop(dropped);
            names
                .names
                .insert(address(&deposit), (stale, "Demo>>dropped".to_string()));
            assert_eq!(names.name(mutator, &deposit), "Demo/Account>>deposit:");
            // And one whose body lives but is another.
            let other = method(&demo, "main:args:");
            names.names.insert(
                address(&deposit),
                (Rc::downgrade(&other), "Demo>>main:args:".to_string()),
            );
            assert_eq!(names.name(mutator, &deposit), "Demo/Account>>deposit:");
        });
    }

    #[test]
    fn a_body_no_mixin_holds_is_unnamed() {
        let (mut interpreter, _) = load(DEMO, None, "Account");
        interpreter.enter(|mutator| {
            let loose = Rc::new(crate::compiler::factory_body(3));
            assert_eq!(name_of(mutator, &loose), "(unnamed)");
        });
    }
}
