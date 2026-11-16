use indoc::{formatdoc, indoc};
use ns_runtime::{Binding, SourceOrigin, resolve};

fn resolve_source(source: &str) -> ns_runtime::ResolvedModule {
    let module = ns_syntax::parse(source).expect("parse");
    resolve(module, source.into(), SourceOrigin::File("test.ns".into())).expect("resolve")
}

#[test]
fn method_temporary_resolves_as_local() {
    let resolved = resolve_source(indoc! {"
        class Foo = Object new (
        ) (
          bar = ( | x | x )
        ) : ()
    "});
    // At least one Local binding exists somewhere in the module (the `x`
    // reference in the method body).
    assert!(
        resolved
            .bindings
            .values()
            .any(|binding| matches!(binding, Binding::Local(_)))
    );
}

#[test]
fn self_send_resolves_to_depth_zero() {
    let resolved = resolve_source(indoc! {"
        class Foo = Object new (
        ) (
          bar = ( ^self baz )
          baz = ( ^1 )
        ) : ()
    "});
    assert!(
        resolved
            .bindings
            .values()
            .any(|binding| matches!(binding, Binding::LexicalSend { depth: 0 }))
    );
}

#[test]
fn super_send_resolves_to_super_send_binding() {
    let resolved = resolve_source(indoc! {"
        class Foo = Object new (
        ) (
          bar = ( ^super baz )
        ) : ()
    "});
    assert!(
        resolved
            .bindings
            .values()
            .any(|binding| *binding == Binding::SuperSend)
    );
}

#[test]
fn outer_named_send_resolves_to_the_exact_depth() {
    let resolved = resolve_source(indoc! {"
        class Outer = Object new (
        ) (
          outerMethod = ( ^1 )
          public class Inner = Object new (
          ) (
            innerMethod = ( ^outer Outer outerMethod )
          ) : ()
        ) : ()
    "});
    assert!(
        resolved
            .bindings
            .values()
            .any(|binding| matches!(binding, Binding::LexicalSend { depth: 1 }))
    );
}

/// A bare reference to a builtin class name that no enclosing declaration
/// declares is §5.7's self send, `self Array`, answered by
/// `Object`'s accessor (§3.2) -- a depth-0 `LexicalSend` at the identifier's
/// span, not a resolve error and not a constant. Locates the exact span of
/// the `Array` identifier by walking the parsed AST directly,
/// rather than scanning `bindings.values()`, which cannot tell which
/// identifier a binding belongs to.
#[test]
fn bare_builtin_class_name_resolves_to_a_self_send_at_its_span() {
    let source = indoc! {"
        class Foo = Object new (
        ) (
          bar = ( | c | c:: Array new. ^c )
        ) : ()
    "};
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module.clone(),
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve must succeed -- a bare builtin class name is not a resolve error");

    let method = module
        .root_class
        .instance_side
        .methods()
        .next()
        .expect("a method");
    let ns_syntax::ast::StatementKind::Expression(ns_syntax::ast::Expression {
        kind: ns_syntax::ast::ExpressionKind::Assign { value, .. },
        ..
    }) = &method.body.statements[0].kind
    else {
        panic!("expected the method's first statement to be `c:: Array new.`");
    };
    let ns_syntax::ast::ExpressionKind::Send(send) = &value.kind else {
        panic!("expected the assignment's value to be a `Send` (`Array new`)");
    };
    let ns_syntax::ast::ExpressionKind::Identifier(receiver_id) = &send.receiver.kind else {
        panic!("expected the send's receiver to be a bare `Identifier` (`Array`)");
    };
    assert_eq!(receiver_id.name, "Array");
    assert!(
        matches!(
            resolved.bindings.get(&receiver_id.span),
            Some(Binding::LexicalSend { depth: 0 })
        ),
        "a bare builtin class name reference must bind a depth-0 self send at its span"
    );
}

#[test]
fn undeclared_outer_class_name_is_a_resolve_error() {
    let module = ns_syntax::parse(indoc! {"
        class Foo = Object new (
        ) (
          bar = ( ^outer Missing baz )
        ) : ()
    "})
    .expect("parse");
    let result = resolve(module, "src".into(), SourceOrigin::File("test.ns".into()));
    assert!(result.is_err());
}

/// §5.9: `N` in `outer N m` is any identifier, so a lowercase one is an
/// outer send too -- resolved when an enclosing class has that name, and a
/// compile-time error when none has, rather than a send of `outer` that
/// nothing understands at run time.
#[test]
fn a_lowercase_outer_class_name_is_an_outer_send() {
    assert_eq!(
        resolve_errors(indoc! {"
            class Foo = Object new () (
              class public = Object new () (
                class Inner = Object new () (
                  bar = ( ^outer public baz )
                ) : ()
                baz = ( ^1 )
              ) : ()
            ) : ()
        "}),
        Vec::<String>::new()
    );
    assert_eq!(
        resolve_errors(indoc! {"
            class Foo = Object new () (
              bar = ( ^outer missing baz )
            ) : ()
        "}),
        vec!["`outer missing` does not name a lexically enclosing class".to_string()]
    );
}

/// §3.6: an activation declares each name once among its parameters and
/// locals, a method's, a block's and a factory's alike. A block's own
/// parameter may still share a name with the enclosing method's, since it
/// is another activation's slot.
#[test]
fn an_activation_declares_each_name_once() {
    let errors = |member: &str| {
        resolve_errors(&formatdoc! {"
            class Foo = Object new () (
              {member}
            ) : ()
        "})
    };
    assert_eq!(
        errors("bar: a = ( | a | ^a )"),
        vec!["method `bar:` declares `a` twice, as a parameter and as a local (§3.6)".to_string()]
    );
    assert_eq!(
        errors("bar: a with: a = ( ^a )"),
        vec![
            "method `bar:with:` declares `a` twice, as a parameter and as a parameter (§3.6)"
                .to_string()
        ]
    );
    assert_eq!(
        errors("bar = ( | a a | ^a )"),
        vec!["method `bar` declares `a` twice, as a local and as a local (§3.6)".to_string()]
    );
    assert_eq!(
        errors("bar = ( ^[:a | | a | a] )"),
        vec!["a block declares `a` twice, as a parameter and as a local (§3.6)".to_string()]
    );
    assert_eq!(
        errors("class Box x: a y: a = Object new () ()"),
        vec![
            "the factory of class `Box` declares `a` twice, as a parameter and as a parameter (§3.6)"
                .to_string()
        ]
    );
    assert_eq!(errors("bar: a = ( ^[:a | a] )"), Vec::<String>::new());
}

/// The resolve errors `source` produces, as messages; empty when it resolves.
fn resolve_errors(source: &str) -> Vec<String> {
    let module = ns_syntax::parse(source).expect("parse");
    match resolve(module, source.into(), SourceOrigin::File("test.ns".into())) {
        Ok(_) => Vec::new(),
        Err(errors) => errors.into_iter().map(|error| error.message).collect(),
    }
}

// Two members of one declaration under one name. `newspeak-spec.pdf` states
// no rule for them; the canonical compiler refuses them
// (`prior-art/newspeak/Newspeak2SqueakCompilation.ns`,
// `checkForDuplicateNames:`, which counts each method's selector, each
// slot's name, each mutable slot's setter and each nested class's name), and
// a mixin's method table cannot hold two members under one selector.

#[test]
fn a_slot_and_a_method_of_one_name_are_a_resolve_error() {
    let errors = resolve_errors(indoc! {"
        class Foo = Object new (
          | x ::= 1. |
        ) (
          x = ( ^2 )
        ) : ()
    "});
    assert_eq!(errors, ["class `Foo` has multiple definitions of `x`"]);
}

/// §6.3.2: a mutable slot's setter is "implicitly defined by the slot
/// declaration", so a method `x:` beside it defines `x:` twice.
#[test]
fn a_mutable_slot_and_a_method_of_its_setters_name_are_a_resolve_error() {
    let errors = resolve_errors(indoc! {"
        class Foo = Object new (
          | x ::= 1. |
        ) (
          x: value = ( ^value )
        ) : ()
    "});
    assert_eq!(errors, ["class `Foo` has multiple definitions of `x:`"]);
}

/// §6.3.2: an immutable slot has "no setter method", and "defining a setter
/// will not impact such an initialization" -- so a method `x:` beside an
/// immutable slot `x` is legitimate.
#[test]
fn an_immutable_slot_and_a_method_of_its_setters_name_resolve() {
    let errors = resolve_errors(indoc! {"
        class Foo = Object new (
          | x = 1. |
        ) (
          x: value = ( ^value )
        ) : ()
    "});
    assert!(errors.is_empty(), "{errors:?}");
}

/// §6: a nested class declaration "implicitly defines a method with the same
/// name", so a method of that name defines it twice.
#[test]
fn a_nested_class_and_a_method_of_one_name_are_a_resolve_error() {
    let errors = resolve_errors(indoc! {"
        class Foo = Object new (
        ) (
          class Inner = Object new () ()
          Inner = ( ^2 )
        ) : ()
    "});
    assert_eq!(errors, ["class `Foo` has multiple definitions of `Inner`"]);
}

/// §6.3 makes the primary factory a member of the metaclass mixin the class
/// side becomes, so a method table has one entry per selector to give it --
/// the same rule as a slot and a method of one name, extended to the
/// factory. Primordial Soup refuses it too (`Intermediates.ns`,
/// `checkNameConflictsForFactory:`).
#[test]
fn a_class_side_may_not_declare_the_factory_selector() {
    let errors = resolve_errors(indoc! {"
        class Conflict usingPlatform: platform = Object new () (
            class Widget new = Object new () (
            ) : (
                public new = ( ^'not a widget' )
            )
        ) : ()
    "});
    assert_eq!(errors, ["class `Widget` has multiple definitions of `new`"]);
}

/// A class written with no factory pattern defaults to `new` (§6's
/// unnumbered introduction, before §6.1 begins), so the collision is refused
/// even when nothing in the source spells the selector out.
#[test]
fn a_class_side_may_not_declare_the_default_factory_selector() {
    let errors = resolve_errors(indoc! {"
        class Conflict usingPlatform: platform = Object new () (
            class Widget = Object new () (
            ) : (
                public new = ( ^'not a widget' )
            )
        ) : ()
    "});
    assert_eq!(errors, ["class `Widget` has multiple definitions of `new`"]);
}

/// A class side under a different selector than the factory's is
/// legitimate -- the check must refuse only the method under the factory's
/// own selector, not every class-side method.
#[test]
fn a_class_side_method_under_another_selector_resolves() {
    let errors = resolve_errors(indoc! {"
        class Widget new = Object new () (
        ) : (
            public make = ( ^'not a widget' )
        )
    "});
    assert!(errors.is_empty(), "{errors:?}");
}

/// The check reads the factory pattern through `method_name`, the same
/// selector-rendering `declared_members` uses for an ordinary method, so it
/// is not unary-only: a keyword factory's selector is refused exactly the
/// same way.
#[test]
fn a_class_side_may_not_declare_a_keyword_factory_selector() {
    let errors = resolve_errors(indoc! {"
        class Boxed with: contents = Object new () (
        ) : (
            public with: contents = ( ^'the class side' )
        )
    "});
    assert_eq!(
        errors,
        ["class `Boxed` has multiple definitions of `with:`"]
    );
}

/// The primary factory selector is a member of the class side
/// (`newspeak-spec.pdf` §6.3: "The class side, together with the primary
/// factory method, defines the mixin for the metaclass"), so §5.7's depth
/// search must find it on the declaring class. Before it did, an implicit
/// `new` written inside a class-side method bound to an *enclosing* class's
/// member `new` at depth 1 and answered that member's value instead of an
/// instance -- silently, at exit status 0. Pinned at the exact span of the
/// `new` identifier, because a depth-0 binding somewhere in the module says
/// nothing about this one.
#[test]
fn an_implicit_factory_send_binds_to_the_declaring_class_not_an_enclosing_member() {
    let source = indoc! {"
        class Outer = Object new (
        ) (
          class Inner = Object new (
          ) (
          ) : (
            make = ( ^new )
          )
          public new = ( ^'the enclosing new' )
        ) : ()
    "};
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = resolve_source(source);

    let inner = module
        .root_class
        .instance_side
        .nested_classes()
        .next()
        .expect("a nested class");
    let class_side = inner
        .class_side
        .as_ref()
        .expect("Inner declares a class side");
    let ns_syntax::ast::StatementKind::Return(ns_syntax::ast::Expression {
        kind: ns_syntax::ast::ExpressionKind::Identifier(factory_send),
        ..
    }) = &class_side
        .methods()
        .next()
        .expect("a method")
        .body
        .statements[0]
        .kind
    else {
        panic!("expected the class-side method's only statement to be `^new`");
    };
    assert_eq!(factory_send.name, "new");
    assert_eq!(
        resolved.bindings.get(&factory_send.span),
        Some(&Binding::LexicalSend { depth: 0 }),
        "an implicit send of the factory selector must bind to the class that \
         declares the factory, not to the enclosing class's member `new`"
    );
}

// --- §6.2's mixin application ---

/// `Applied`, two classes deep inside `Host`, with `inheritance` as its whole
/// inheritance list. `Host` declares a `Mixin` slot, so a bare `Mixin` has a
/// member to find two levels out and a named `outer Host` has a class to name.
fn module_applying(inheritance: &str) -> String {
    formatdoc! {"
        class Host using: platform = Object new (
          | private Mixin = platform Mixin. |
        ) (
          class Middle = Object new (
          ) (
            class Applied = {inheritance} (
            ) ()
          ) : ()
        ) : ()
    "}
}

/// The doubly nested `Applied` of [`module_applying`].
fn applied_class(module: &ns_syntax::ast::Module) -> &ns_syntax::ast::ClassDeclaration {
    module
        .root_class
        .instance_side
        .nested_classes()
        .next()
        .expect("a nested class")
        .instance_side
        .nested_classes()
        .next()
        .expect("a nested class")
}

/// The binding recorded at the receiver of `Applied`'s superclass clause, when
/// that clause is written `<inheritance_clause> new`. `None` means the
/// resolver recorded nothing there and left the name to evaluation.
fn binding_in_superclass_position(inheritance_clause: &str) -> Option<Binding> {
    let source = module_applying(&format!("{inheritance_clause} new"));
    let module = ns_syntax::parse(&source).expect("parse");
    let resolved = resolve_source(&source);
    let clause = applied_class(&module)
        .superclass
        .as_ref()
        .expect("a superclass clause");
    resolved.bindings.get(&clause.receiver.span).copied()
}

/// The binding recorded at the receiver of the one mixin `Applied` applies,
/// when that mixin is written `Object new <: <inheritance_clause> new`.
fn binding_in_mixin_position(inheritance_clause: &str) -> Option<Binding> {
    let source = module_applying(&format!("Object new <: {inheritance_clause} new"));
    let module = ns_syntax::parse(&source).expect("parse");
    let resolved = resolve_source(&source);
    let clause = &applied_class(&module).mixin_applications[0];
    resolved.bindings.get(&clause.receiver.span).copied()
}

/// §6.2's `mixinAppSuffix` is built from §6.1's very `inheritanceClause`
/// production, so a `<:` clause's receiver must bind exactly as the same
/// receiver binds in superclass position. Every spelling
/// `resolve_superclass_clause` distinguishes is resolved twice, once in each
/// position, and the two must agree.
#[test]
fn a_mixin_clause_receiver_binds_exactly_as_a_superclass_receiver_does() {
    for (inheritance_clause, expected) in [
        // A member of a class two levels out.
        ("Mixin", Some(Binding::LexicalSend { depth: 1 })),
        // An explicitly named enclosing class.
        ("outer Host Mixin", Some(Binding::LexicalSend { depth: 1 })),
        (
            "outer Middle Mixin",
            Some(Binding::LexicalSend { depth: 0 }),
        ),
        ("self Mixin", Some(Binding::LexicalSend { depth: 0 })),
        ("super Mixin", Some(Binding::SuperSend)),
        // A builtin class name no enclosing class declares: §5.7's self
        // send, answered by `Object`'s accessor (§3.2) at run time.
        ("Object", Some(Binding::LexicalSend { depth: 0 })),
        // Declared nowhere: §5.7's implicit-receiver send, found at run time.
        ("Unknown", Some(Binding::LexicalSend { depth: 0 })),
    ] {
        assert_eq!(
            binding_in_superclass_position(inheritance_clause),
            expected,
            "`{inheritance_clause}` in superclass position"
        );
        assert_eq!(
            binding_in_mixin_position(inheritance_clause),
            expected,
            "`{inheritance_clause}` as a mixin application"
        );
    }
}

/// Every clause of a multi-mixin application is resolved, in written order.
#[test]
fn every_mixin_clause_of_an_application_is_resolved() {
    let source = module_applying("Object new <: Mixin new <: super Mixin new <: Object new");
    let module = ns_syntax::parse(&source).expect("parse");
    let resolved = resolve_source(&source);
    let clauses = &applied_class(&module).mixin_applications;
    assert_eq!(clauses.len(), 3);
    let bindings: Vec<Option<Binding>> = clauses
        .iter()
        .map(|clause| resolved.bindings.get(&clause.receiver.span).copied())
        .collect();
    assert_eq!(
        bindings,
        vec![
            Some(Binding::LexicalSend { depth: 1 }),
            Some(Binding::SuperSend),
            Some(Binding::LexicalSend { depth: 0 }),
        ]
    );
}

/// A mixin clause's receiver is held to the same rules, so `outer Missing` is
/// the same resolve error there as it is in superclass position.
#[test]
fn an_undeclared_outer_class_in_a_mixin_clause_is_a_resolve_error() {
    let errors = resolve_errors(&module_applying("Object new <: outer Missing Mixin new"));
    assert!(
        errors
            .iter()
            .any(|message| message.contains("`outer Missing` does not name")),
        "{errors:?}"
    );
}

/// §6: the primary factory's parameters "are in scope in the instance
/// initializer but not in the rest of the class body", and a lazy slot is
/// declared in the class body (§6.3.3), so `seed` in its initializer is the
/// implicit-receiver send `self seed`, not the factory's parameter.
#[test]
fn a_lazy_slot_initializer_does_not_see_the_factory_parameters() {
    let source = indoc! {"
        class Foo seed: seed = Object new (
            | eager = seed. |
        ) (
            lazy public later = seed.
        ) : ()
    "};
    let resolved = resolve_source(source);
    let eager = source.find("= seed.").expect("the eager initializer") + 2;
    let later = source.rfind("= seed.").expect("the lazy initializer") + 2;
    let binding_at = |start: usize| {
        resolved
            .bindings
            .iter()
            .find(|(span, _)| span.start as usize == start)
            .map(|(_, binding)| *binding)
    };
    assert!(matches!(binding_at(eager), Some(Binding::Local(_))));
    assert!(matches!(
        binding_at(later),
        Some(Binding::LexicalSend { depth: 0 })
    ));
}

/// A module whose root class is `class R{factory} = {clauses} ()`: the
/// header's factory before the `=` and the clauses after it, as written.
fn module_with_root(factory: &str, clauses: &str) -> String {
    formatdoc! {"
        class R{factory} = {clauses} () (
        ) : ()
    "}
}

/// The root rule is checked by the resolver, for every module root, so a
/// module whose artifact `build` could never write never runs either
/// (`docs/design/object-model.md`, "The root rule"). It leaves every
/// combination of `Object` and `Value` that works, and nothing else.
#[test]
fn every_working_combination_of_object_and_value_resolves() {
    for clauses in [
        "",
        "Object",
        "Object new",
        "Value",
        "Value new",
        "Object <: Value",
        "Object new <: Value",
    ] {
        let source = if clauses.is_empty() {
            indoc! {"
                class R = () (
                ) : ()
            "}
            .to_string()
        } else {
            module_with_root("", clauses)
        };
        assert_eq!(resolve_errors(&source), Vec::<String>::new(), "`{clauses}`");
    }
}

/// The primary factory before the `=` is not a clause, so the rule leaves it
/// free.
#[test]
fn the_root_rule_leaves_the_primary_factory_free() {
    for (factory, clauses) in [
        (" packageUsing: manifest", "Object new"),
        (" usingPlatform: platform", "Value new"),
        (" new", "Object <: Value"),
    ] {
        let source = module_with_root(factory, clauses);
        assert_eq!(
            resolve_errors(&source),
            Vec::<String>::new(),
            "`{factory} = {clauses}`"
        );
    }
}

/// Every other root clause is one resolve error, spanning the clause at
/// fault as written, with the tail the instantiation refusal names.
#[test]
fn every_other_root_clause_is_one_resolve_error_at_that_clause() {
    let rule = "a module's root class may inherit only Object or Value, or write Object <: Value";
    // The clauses after the `=`, the offending clause as written, whether it
    // is the last occurrence of that text rather than the first, and the tail.
    let cases = [
        (
            "Array new",
            "Array new",
            false,
            "its superclass clause names Array",
        ),
        (
            "Missing new",
            "Missing new",
            false,
            "its superclass clause names Missing",
        ),
        (
            "Top new",
            "Top new",
            false,
            "its superclass clause names Top",
        ),
        (
            "self Object new",
            "self Object new",
            false,
            "its superclass clause names self Object",
        ),
        (
            "outer Missing Object new",
            "outer Missing Object new",
            false,
            "its superclass clause names outer Missing Object",
        ),
        (
            "Object foo",
            "Object foo",
            false,
            "its superclass clause sends foo to Object, not new",
        ),
        (
            "Object new: 3",
            "Object new: 3",
            false,
            "its superclass clause sends new: to Object, not new",
        ),
        (
            "Value + 3",
            "Value + 3",
            false,
            "its superclass clause sends + to Value, not new",
        ),
        (
            "Object <: Array",
            "Array",
            false,
            "its mixin application clause 1 names Array",
        ),
        (
            "Object <: Missing",
            "Missing",
            false,
            "its mixin application clause 1 names Missing",
        ),
        (
            "Object <: self Value",
            "self Value",
            false,
            "its mixin application clause 1 names self Value",
        ),
        (
            "Object <: Value new",
            "Value new",
            false,
            "its mixin application clause 1 sends new to Value",
        ),
        // `Value <: Value`'s one `<:` clause is right; its superclass is not.
        (
            "Value <: Value",
            "Value",
            false,
            "it applies Value to Value",
        ),
        (
            "Value new <: Value",
            "Value new",
            false,
            "it applies Value to Value",
        ),
        (
            "Object <: Value <: Value",
            "Value",
            true,
            "its mixin application clause 2 names Value",
        ),
    ];
    for (clauses, offending, last, tail) in cases {
        let source = module_with_root("", clauses);
        let module = ns_syntax::parse(&source).expect("parse");
        let errors = match resolve(
            module,
            source.as_str().into(),
            SourceOrigin::File("test.ns".into()),
        ) {
            Ok(_) => panic!("`{clauses}` resolved"),
            Err(errors) => errors,
        };
        assert_eq!(errors.len(), 1, "`{clauses}`");
        assert_eq!(
            errors[0].message,
            format!("{rule}, and {tail}"),
            "`{clauses}`"
        );
        let start = if last {
            source.rfind(offending)
        } else {
            source.find(offending)
        }
        .expect("the offending clause");
        let span = errors[0].span;
        assert_eq!(
            (span.start as usize, span.end as usize),
            (start, start + offending.len()),
            "`{clauses}` spans {:?}",
            &source[span.start as usize..span.end as usize]
        );
    }
}

/// Parentheses change no resolution: `(outer Outer) yourself` is the
/// `outer N m` send `outer Outer yourself` is.
#[test]
fn a_parenthesised_outer_reference_resolves_as_the_bare_one() {
    let parenthesised = indoc! {"
        class Outer = (
        ) (
            class Inner = (
            ) (
                public answer = ( ^(outer Outer) yourself )
            )
        )
    "};
    let bare = parenthesised.replace("(outer Outer)", "outer Outer");
    let parenthesised_binding = bindings_of_yourself_and_its_receiver(parenthesised);
    assert!(
        parenthesised_binding.contains("LexicalSend { depth: 1 }"),
        "{parenthesised_binding}"
    );
    assert_eq!(
        parenthesised_binding,
        bindings_of_yourself_and_its_receiver(&bare)
    );
}

/// What the resolution of `source` records for the `yourself` send in
/// `Inner>>answer` and for its receiver, each looked up by its shape's span.
/// The resolver binds the two-hop `outer N m` on the send itself and nothing
/// on its receiver, so both are answered; the `Debug` text holds no span, so
/// two sources whose spans differ compare equal.
fn bindings_of_yourself_and_its_receiver(source: &str) -> String {
    let resolved = resolve_source(source);
    let inner = resolved
        .module
        .root_class
        .instance_side
        .nested_classes()
        .next()
        .expect("a nested class");
    let ns_syntax::ast::StatementKind::Return(returned) = &inner
        .instance_side
        .methods()
        .next()
        .expect("a method")
        .body
        .statements[0]
        .kind
    else {
        panic!("expected `answer` to return the send of `yourself`");
    };
    let ns_syntax::ast::ExpressionKind::Send(send) = &returned.kind else {
        panic!("expected `answer` to return a send");
    };
    assert_eq!(
        send.message.selector,
        ns_syntax::ast::Selector::Unary("yourself".to_string())
    );
    format!(
        "send: {:?}, receiver: {:?}",
        resolved.bindings.get(&returned.kind.span()),
        resolved.bindings.get(&send.receiver.kind.span())
    )
}

/// The parser's `synthesized` flag tells the implicit receiver from a
/// written `self`, not its span: on a tree without positions every span is
/// empty, and a written `self foo: 1` still sends to `self` (§5.8) rather
/// than searching the enclosing classes for `foo:`.
#[test]
fn an_explicit_self_send_stays_explicit_on_a_tree_without_positions() {
    let source = indoc! {"
        class Outer = () (
            public foo: x = ( )
            class Inner = () (
                public answer = ( self foo: 1 )
            )
        )
    "};
    let module = ns_syntax::parse(source).expect("parse").without_positions();
    let resolved =
        resolve(module, source.into(), SourceOrigin::File("test.ns".into())).expect("resolve");
    let sends: Vec<String> = resolved
        .bindings
        .values()
        .filter(|binding| matches!(binding, Binding::LexicalSend { .. }))
        .map(|binding| format!("{binding:?}"))
        .collect();
    assert_eq!(sends, ["LexicalSend { depth: 0 }"]);
}

/// A superclass clause may apply at most 1,024 mixins: 1,025 is one resolve
/// error naming the limit, and 1,024 resolves.
#[test]
fn a_superclass_clause_of_1025_mixins_is_refused_at_resolve() {
    let module_applying = |count: usize| {
        let applications = " <: Marker".repeat(count);
        formatdoc! {"
            class Outer = Object new () (
              class Marker = Object new () () : ()
              class Wide = Object{applications}.
            ) : ()
        "}
    };
    assert_eq!(
        resolve_errors(&module_applying(1025)),
        ["class `Wide` applies 1025 mixins in its superclass clause; at most 1024 are allowed"]
    );
    assert_eq!(resolve_errors(&module_applying(1024)), Vec::<String>::new());
}
