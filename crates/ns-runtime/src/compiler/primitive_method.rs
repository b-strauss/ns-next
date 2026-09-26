//! Primitive methods (`docs/design/dispatch.md`, "Primitive methods"): a
//! kernel method whose whole body forwards `self` and its own parameters to
//! one mirror row runs that row's leaf when it is sent, without an
//! activation, and is activated -- its body running the failure block --
//! only when the leaf declines. Cog's primitive methods, with the primitive
//! recognized from a body that means exactly what it says rather than
//! declared by a pragma that replaces what the body says.
//!
//! The same holds for §3.2's accessors for the predefined classes on
//! `Object`: a kernel method whose whole body is `^outer Kernel <a kernel
//! class>` answers that builtin class and is never activated
//! (`docs/design/core-library.md`, "The predefined classes are accessors
//! on `Object`").
//!
//! And for quick methods, Squeak's and Cog's name for a method whose whole
//! body answers a constant or a parameter: a kernel method whose body is
//! `^true`, `^false`, `^nil`, `^self`, `^<parameter>`, `^<parameter> value`
//! or `^<parameter> cull: self` answers without an activation -- the last two
//! as the block to activate, which is how `True`, `False` and the nil
//! protocol are written (`docs/design/core-library.md`, "`True`
//! and `False` are quick methods").

use std::collections::HashMap;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::builtins::primitive::MirrorPrimitive;
use crate::compiler::control_flow;
use crate::mixin::{MethodFastPath, QuickConstant};
use crate::types::{Binding, BuiltinClass};

/// What a send of `declaration` does without an activation, if anything:
/// the mirror row it forwards to (`recognize_mirror_forwarder`) or, in the
/// kernel alone, the builtin class it answers
/// (`recognize_builtin_class_accessor`) or what its quick body answers
/// (`recognize_quick_method`). `kernel` is the module's
/// `ResolvedModule.kernel`.
pub(crate) fn recognize(
    declaration: &ast::MethodDeclaration,
    bindings: &HashMap<Span, Binding>,
    kernel: bool,
) -> Option<MethodFastPath> {
    // The mirror forwarder needs no `kernel` check of its own: only
    // `resolve_kernel`'s bindings carry the `MirrorSend` it requires.
    if let Some(row) = recognize_mirror_forwarder(declaration, bindings) {
        return Some(MethodFastPath::Mirror(row));
    }
    if !kernel {
        return None;
    }
    recognize_builtin_class_accessor(declaration, bindings)
        .map(MethodFastPath::BuiltinClass)
        .or_else(|| recognize_quick_method(declaration, bindings))
}

/// A quick method: a kernel method whose whole body is `^true`, `^false`,
/// `^nil`, `^self`, `^<parameter>`, `^<parameter> value` or
/// `^<parameter> cull: self` runs without an activation, as Squeak's and
/// Cog's quick methods do -- the last two extend theirs by the block
/// activation `True`'s, `False`'s and the nil protocol's methods consist of. `True` and `False`
/// are written this way (`docs/design/core-library.md`, "`True` and `False`
/// are quick methods"): an activation of `True>>ifTrue:` would count
/// against the depth limit under the reference lowering, which sends the
/// conditional, and not under the optimized one, which inlines it
/// (`docs/design/compiler.md`, "How the limit counts").
///
/// Not switched off by `NS_NEXT_NO_PRIMITIVE_METHODS`: that switch measures
/// the mirror leaves' fast path, and a quick method runs no leaf.
fn recognize_quick_method(
    declaration: &ast::MethodDeclaration,
    bindings: &HashMap<Span, Binding>,
) -> Option<MethodFastPath> {
    let body = &declaration.body;
    if !body.temporaries.is_empty() || body.statements.len() != 1 {
        return None;
    }
    let ast::Statement::Return(expression) = &body.statements[0] else {
        return None;
    };
    let parameters = parameter_names(&declaration.pattern);
    match expression {
        ast::Expression::Literal(ast::Literal::Boolean { value: true, .. }) => {
            Some(MethodFastPath::Constant(QuickConstant::True))
        }
        ast::Expression::Literal(ast::Literal::Boolean { value: false, .. }) => {
            Some(MethodFastPath::Constant(QuickConstant::False))
        }
        ast::Expression::Literal(ast::Literal::Nil { .. }) => {
            Some(MethodFastPath::Constant(QuickConstant::Nil))
        }
        ast::Expression::Identifier(_) if is_self(expression) => Some(MethodFastPath::Receiver),
        ast::Expression::Identifier(identifier) => {
            parameter_position(identifier, &parameters, bindings).map(MethodFastPath::Parameter)
        }
        ast::Expression::Send(send) => {
            let ast::Expression::Identifier(identifier) = &send.receiver else {
                return None;
            };
            let shape: fn(u8) -> MethodFastPath =
                match (&send.message.selector, &send.message.arguments[..]) {
                    (ast::Selector::Unary(selector), []) if selector == "value" => {
                        MethodFastPath::ValueOfParameter
                    }
                    (ast::Selector::Keyword(selector), [argument])
                        if selector == "cull:" && is_self(argument) =>
                    {
                        MethodFastPath::CullParameter
                    }
                    _ => return None,
                };
            parameter_position(identifier, &parameters, bindings).map(shape)
        }
        _ => None,
    }
}

/// The position of the parameter `identifier` names, when the resolver bound
/// it to that very parameter -- the binding, not the name alone, says it is
/// not something that shadows it.
fn parameter_position(
    identifier: &ast::Identifier,
    parameters: &[&str],
    bindings: &HashMap<Span, Binding>,
) -> Option<u8> {
    let position = parameters
        .iter()
        .position(|name| *name == identifier.name)?;
    let bound_to_it = matches!(
        bindings.get(&identifier.span),
        Some(Binding::Local(local)) if usize::from(*local) == position
    );
    if !bound_to_it {
        return None;
    }
    u8::try_from(position).ok()
}

/// The builtin class `declaration` answers, when its body is exactly
/// `^outer Kernel <Name>` with `<Name>` a kernel class and the send resolved
/// lexically: §3.2's accessors on `Object`. The caller has checked that this
/// is the kernel -- in a user module `outer Kernel` names whatever that
/// module's own `Kernel` is. A parameter, a temporary, a second statement, a
/// message with arguments, or a name that is no kernel class makes it an
/// ordinary method; so does `Top`, which §3.2 lets only `Object` name, as
/// its superclass, and which has no accessor. `Platform` and its members'
/// classes are no kernel classes at all: they live in the shipped
/// `Platform.ns`, which a program reaches only through the platform it is
/// handed.
///
/// Not switched off by `NS_NEXT_NO_PRIMITIVE_METHODS`: that switch measures
/// the mirror leaves' fast path, and an accessor runs no leaf.
fn recognize_builtin_class_accessor(
    declaration: &ast::MethodDeclaration,
    bindings: &HashMap<Span, Binding>,
) -> Option<BuiltinClass> {
    if !matches!(declaration.pattern, ast::MessagePattern::Unary { .. }) {
        return None;
    }
    let body = &declaration.body;
    if !body.temporaries.is_empty() || body.statements.len() != 1 {
        return None;
    }
    let ast::Statement::Return(ast::Expression::Send(send)) = &body.statements[0] else {
        return None;
    };
    if crate::resolver::sends::outer_class_name(&send.receiver) != Some("Kernel") {
        return None;
    }
    let ast::Selector::Unary(name) = &send.message.selector else {
        return None;
    };
    if !matches!(bindings.get(&send.span), Some(Binding::LexicalSend { .. })) {
        return None;
    }
    crate::interpreter::kernel::kernel_kind_named(name).filter(|kind| *kind != BuiltinClass::Top)
}

/// The row `declaration` forwards to, when its body is exactly
/// `^vmMirror <row>: self <its parameters, in order> ifFail: [<anything>]`
/// and the send is a recognized mirror send (`recognize_mirror_send`, which
/// only `resolve_kernel`'s bindings can satisfy). Anything else -- a
/// temporary, a second statement, a subject other than `self`, a parameter
/// out of place, an arity the row does not have -- is an ordinary method.
fn recognize_mirror_forwarder(
    declaration: &ast::MethodDeclaration,
    bindings: &HashMap<Span, Binding>,
) -> Option<MirrorPrimitive> {
    let body = &declaration.body;
    if !body.temporaries.is_empty() || body.statements.len() != 1 {
        return None;
    }
    let ast::Statement::Return(ast::Expression::Send(send)) = &body.statements[0] else {
        return None;
    };
    let (row, _) = control_flow::recognize_mirror_send(bindings, send)?;
    // Read only once a kernel mirror send is found, so that no method outside
    // the kernel ever consults the environment.
    if disabled() {
        return None;
    }
    let arguments = &send.message.arguments;
    let forwarded = &arguments[..arguments.len() - 1];
    let [subject, rest @ ..] = forwarded else {
        return None;
    };
    if !is_self(subject) {
        return None;
    }
    let parameters = parameter_names(&declaration.pattern);
    if rest.len() != parameters.len() {
        return None;
    }
    // The leaf receives the send's own arguments in place of the forwarded
    // ones, so each forwarded argument must be the parameter at its own
    // position: the name alone is not enough, since the binding is what says
    // the resolver found that parameter and not something that shadows it.
    let in_order = rest
        .iter()
        .zip(&parameters)
        .enumerate()
        .all(|(position, (argument, name))| {
            let ast::Expression::Identifier(identifier) = argument else {
                return false;
            };
            identifier.name == *name
                && matches!(
                    bindings.get(&identifier.span),
                    Some(Binding::Local(local)) if usize::from(*local) == position
                )
        });
    in_order.then_some(row)
}

/// `NS_NEXT_NO_PRIMITIVE_METHODS=1` compiles the kernel without primitive
/// methods, so a measurement can run the same binary with and without them
/// (`docs/design/execution.md`, "What the core library's move cost"). Read
/// once per process. **For measuring only**: without primitive methods the
/// two lowerings no longer count activations alike
/// (`docs/design/compiler.md`, "How the limit counts"), so the goldens'
/// overflow comparisons are not valid under it.
pub(crate) fn disabled() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| {
        std::env::var("NS_NEXT_NO_PRIMITIVE_METHODS").is_ok_and(|value| value == "1")
    })
}

/// An explicit `self`. The resolver's implicit-receiver marker is a `self`
/// too, but only ever in a receiver position, never as an argument.
fn is_self(expression: &ast::Expression) -> bool {
    matches!(expression, ast::Expression::Identifier(identifier) if identifier.name == "self")
}

/// The method's parameters in declaration order -- the order the resolver
/// numbers them in (`resolver::scope::LocalScope::new`).
fn parameter_names(pattern: &ast::MessagePattern) -> Vec<&str> {
    match pattern {
        ast::MessagePattern::Unary { .. } => Vec::new(),
        ast::MessagePattern::Binary { parameter, .. } => vec![parameter.name.as_str()],
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|(_, parameter)| parameter.name.as_str())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::primitive::mirror_primitive_named;
    use indoc::formatdoc;

    fn module_source(method: &str) -> String {
        formatdoc! {"
            Newspeak3
            'T'
            class Kernel vmMirror: theMirror = Object new (
              | private vmMirror = theMirror. |
            ) (
              class C = Object new () (
                {method}
              )
            ) : ()
        "}
    }

    /// `method` as the one method of a nested class in a kernel-shaped
    /// module resolved by `resolve_kernel`, and what `recognize` answers for
    /// it.
    fn fast_path_in_the_kernel(method: &str) -> Option<MethodFastPath> {
        let source = module_source(method);
        let module = ns_syntax::parse(&source).expect("parse");
        let Ok(resolved) = crate::resolver::resolve_kernel(
            module,
            std::rc::Rc::from(source.as_str()),
            crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
        ) else {
            panic!("the kernel-shaped module resolves");
        };
        assert!(
            resolved.kernel,
            "resolve_kernel marks its module as the kernel"
        );
        let nested = &resolved.module.root_class.instance_side.nested_classes[0];
        recognize(
            &nested.instance_side.methods[0],
            &resolved.bindings,
            resolved.kernel,
        )
    }

    /// `fast_path_in_the_kernel`, as a mirror row index.
    fn recognized_in_the_kernel(method: &str) -> Option<usize> {
        match fast_path_in_the_kernel(method) {
            Some(MethodFastPath::Mirror(row)) => Some(row.index()),
            Some(other) => panic!("a mirror forwarder was recognized as {other:?}"),
            None => None,
        }
    }

    /// `method` in the same module resolved by `resolve`, as a user module,
    /// and what `recognize` answers for it.
    fn fast_path_outside_the_kernel(method: &str) -> Option<MethodFastPath> {
        let source = module_source(method);
        let module = ns_syntax::parse(&source).expect("parse");
        let resolved = crate::resolve(
            module,
            std::rc::Rc::from(source.as_str()),
            crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
        )
        .expect("resolve");
        assert!(!resolved.kernel, "a user module is not the kernel");
        let nested = &resolved.module.root_class.instance_side.nested_classes[0];
        recognize(
            &nested.instance_side.methods[0],
            &resolved.bindings,
            resolved.kernel,
        )
    }

    fn row(selector: &str) -> Option<usize> {
        let found = mirror_primitive_named(selector).map(MirrorPrimitive::index);
        assert!(found.is_some(), "{selector} is a mirror row");
        found
    }

    #[test]
    fn recognizes_a_unary_forwarder() {
        assert_eq!(
            recognized_in_the_kernel("size = ( ^vmMirror stringSize: self ifFail: [^super size] )"),
            row("stringSize:ifFail:")
        );
    }

    #[test]
    fn recognizes_a_binary_forwarder() {
        assert_eq!(
            recognized_in_the_kernel(
                ", other = ( ^vmMirror string: self concatenate: other ifFail: [^super , other] )"
            ),
            row("string:concatenate:ifFail:")
        );
    }

    #[test]
    fn recognizes_a_keyword_forwarder() {
        assert_eq!(
            recognized_in_the_kernel(
                "at: index put: value = ( ^vmMirror collection: self at: index put: value ifFail: [^nil] )"
            ),
            row("collection:at:put:ifFail:")
        );
    }

    #[test]
    fn does_not_recognize_parameters_out_of_order() {
        assert_eq!(
            recognized_in_the_kernel(
                "at: index put: value = ( ^vmMirror collection: self at: value put: index ifFail: [^nil] )"
            ),
            None
        );
    }

    #[test]
    fn does_not_recognize_a_subject_other_than_self() {
        assert_eq!(
            recognized_in_the_kernel("size = ( ^vmMirror stringSize: 3 ifFail: [^nil] )"),
            None
        );
    }

    #[test]
    fn does_not_recognize_a_method_with_a_temporary() {
        assert_eq!(
            recognized_in_the_kernel(
                "size = ( | temporary | ^vmMirror stringSize: self ifFail: [^nil] )"
            ),
            None
        );
    }

    #[test]
    fn does_not_recognize_a_method_with_more_than_one_statement() {
        assert_eq!(
            recognized_in_the_kernel(
                "size = ( self foo. ^vmMirror stringSize: self ifFail: [^nil] )"
            ),
            None
        );
    }

    #[test]
    fn does_not_recognize_a_body_that_does_not_return_the_send() {
        assert_eq!(
            recognized_in_the_kernel("size = ( vmMirror stringSize: self ifFail: [^nil] )"),
            None
        );
    }

    #[test]
    fn does_not_recognize_a_failure_block_that_takes_a_parameter() {
        assert_eq!(
            recognized_in_the_kernel(
                "size = ( ^vmMirror stringSize: self ifFail: [:reason | ^nil] )"
            ),
            None
        );
    }

    #[test]
    fn does_not_recognize_a_row_whose_arity_is_not_the_method_s() {
        assert_eq!(
            recognized_in_the_kernel(", other = ( ^vmMirror stringSize: self ifFail: [^nil] )"),
            None
        );
    }

    /// Outside shipped source there is no `MirrorSend`, so the same text is
    /// an ordinary method.
    #[test]
    fn does_not_recognize_a_forwarder_outside_the_kernel() {
        assert_eq!(
            fast_path_outside_the_kernel(
                "size = ( ^vmMirror stringSize: self ifFail: [^super size] )"
            ),
            None
        );
    }

    #[test]
    fn recognizes_a_builtin_class_accessor() {
        assert_eq!(
            fast_path_in_the_kernel("String = ( ^outer Kernel String )"),
            Some(MethodFastPath::BuiltinClass(BuiltinClass::String))
        );
    }

    /// The name sent to the kernel decides the class, not the method's own
    /// selector.
    #[test]
    fn an_accessor_answers_the_class_it_names_whatever_its_selector() {
        assert_eq!(
            fast_path_in_the_kernel("text = ( ^outer Kernel String )"),
            Some(MethodFastPath::BuiltinClass(BuiltinClass::String))
        );
    }

    /// §3.2 lets only `Object` name `Top`, and `Object` names it as its
    /// superclass, not through an accessor.
    #[test]
    fn does_not_recognize_an_accessor_for_top() {
        assert_eq!(fast_path_in_the_kernel("Top = ( ^outer Kernel Top )"), None);
    }

    /// No program is handed a `Platform` except as its factory's argument,
    /// so `Object` has no accessor for it, and none is ever recognized:
    /// `Platform` lives in the shipped `Platform.ns`, not in `Kernel.ns`, so
    /// the name is no kernel class at all.
    #[test]
    fn does_not_recognize_an_accessor_for_platform() {
        assert_eq!(
            fast_path_in_the_kernel("Platform = ( ^outer Kernel Platform )"),
            None
        );
    }

    #[test]
    fn does_not_recognize_an_accessor_for_a_name_that_is_no_kernel_class() {
        assert_eq!(
            fast_path_in_the_kernel("Widget = ( ^outer Kernel Widget )"),
            None
        );
        assert_eq!(
            fast_path_in_the_kernel("VmMirror = ( ^outer Kernel VmMirror )"),
            None
        );
    }

    #[test]
    fn does_not_recognize_an_accessor_through_another_enclosing_class() {
        assert_eq!(
            fast_path_in_the_kernel("String = ( ^outer C String )"),
            None
        );
    }

    #[test]
    fn does_not_recognize_an_accessor_with_a_temporary_or_a_second_statement() {
        assert_eq!(
            fast_path_in_the_kernel("String = ( | temporary | ^outer Kernel String )"),
            None
        );
        assert_eq!(
            fast_path_in_the_kernel("String = ( self foo. ^outer Kernel String )"),
            None
        );
    }

    #[test]
    fn does_not_recognize_an_accessor_that_takes_a_parameter() {
        assert_eq!(
            fast_path_in_the_kernel("String: value = ( ^outer Kernel String )"),
            None
        );
    }

    #[test]
    fn does_not_recognize_an_accessor_that_does_not_return_the_class() {
        assert_eq!(
            fast_path_in_the_kernel("String = ( outer Kernel String )"),
            None
        );
    }

    /// Outside shipped source, `outer Kernel` names whatever the module's
    /// own `Kernel` is, so the same text is an ordinary method.
    #[test]
    fn does_not_recognize_an_accessor_outside_the_kernel() {
        assert_eq!(
            fast_path_outside_the_kernel("String = ( ^outer Kernel String )"),
            None
        );
    }

    #[test]
    fn recognizes_a_constant_answer() {
        assert_eq!(
            fast_path_in_the_kernel("not = ( ^false )"),
            Some(MethodFastPath::Constant(QuickConstant::False))
        );
        assert_eq!(
            fast_path_in_the_kernel("or: alternative = ( ^true )"),
            Some(MethodFastPath::Constant(QuickConstant::True))
        );
        assert_eq!(
            fast_path_in_the_kernel("ifFalse: falseBlock = ( ^nil )"),
            Some(MethodFastPath::Constant(QuickConstant::Nil))
        );
    }

    #[test]
    fn recognizes_a_parameter_answer() {
        assert_eq!(
            fast_path_in_the_kernel("& other = ( ^other )"),
            Some(MethodFastPath::Parameter(0))
        );
        assert_eq!(
            fast_path_in_the_kernel("at: first put: second = ( ^second )"),
            Some(MethodFastPath::Parameter(1))
        );
    }

    #[test]
    fn recognizes_a_parameter_s_value() {
        assert_eq!(
            fast_path_in_the_kernel("ifTrue: trueBlock = ( ^trueBlock value )"),
            Some(MethodFastPath::ValueOfParameter(0))
        );
        assert_eq!(
            fast_path_in_the_kernel(
                "ifTrue: trueBlock ifFalse: falseBlock = ( ^falseBlock value )"
            ),
            Some(MethodFastPath::ValueOfParameter(1))
        );
    }

    /// Only the three shapes: another literal, `self`, another selector or
    /// an argument to `value` is an ordinary method.
    #[test]
    fn does_not_recognize_other_trivial_bodies() {
        assert_eq!(fast_path_in_the_kernel("size = ( ^3 )"), None);
        assert_eq!(fast_path_in_the_kernel("text = ( ^'true' )"), None);
        assert_eq!(
            fast_path_in_the_kernel("ifTrue: trueBlock = ( ^trueBlock yourself )"),
            None
        );
        assert_eq!(
            fast_path_in_the_kernel("ifTrue: trueBlock = ( ^trueBlock value: 1 )"),
            None
        );
        assert_eq!(
            fast_path_in_the_kernel("ifTrue: trueBlock = ( ^trueBlock value value )"),
            None
        );
    }

    #[test]
    fn does_not_recognize_a_quick_body_with_a_temporary_or_a_second_statement() {
        assert_eq!(
            fast_path_in_the_kernel("not = ( | temporary | ^false )"),
            None
        );
        assert_eq!(fast_path_in_the_kernel("not = ( self foo. ^false )"), None);
        assert_eq!(
            fast_path_in_the_kernel("ifTrue: trueBlock = ( self foo. ^trueBlock value )"),
            None
        );
    }

    #[test]
    fn does_not_recognize_a_quick_body_that_does_not_return() {
        assert_eq!(fast_path_in_the_kernel("not = ( false )"), None);
        assert_eq!(
            fast_path_in_the_kernel("ifTrue: trueBlock = ( trueBlock value )"),
            None
        );
    }

    /// A name that is not the method's own parameter -- here the enclosing
    /// kernel's slot -- is no parameter answer.
    #[test]
    fn does_not_recognize_a_name_that_is_not_a_parameter() {
        assert_eq!(fast_path_in_the_kernel("mirror = ( ^vmMirror )"), None);
    }

    /// Outside shipped source a quick body is an ordinary method, activated
    /// like any other.
    #[test]
    fn does_not_recognize_a_quick_method_outside_the_kernel() {
        assert_eq!(fast_path_outside_the_kernel("not = ( ^false )"), None);
        assert_eq!(fast_path_outside_the_kernel("& other = ( ^other )"), None);
        assert_eq!(
            fast_path_outside_the_kernel("ifTrue: trueBlock = ( ^trueBlock value )"),
            None
        );
    }

    #[test]
    fn recognizes_a_receiver_answer() {
        assert_eq!(
            fast_path_in_the_kernel("yourself = ( ^self )"),
            Some(MethodFastPath::Receiver)
        );
        assert_eq!(
            fast_path_in_the_kernel("ifNil: block = ( ^self )"),
            Some(MethodFastPath::Receiver)
        );
    }

    #[test]
    fn recognizes_a_parameter_culled_with_the_receiver() {
        assert_eq!(
            fast_path_in_the_kernel("ifNotNil: block = ( ^block cull: self )"),
            Some(MethodFastPath::CullParameter(0))
        );
        assert_eq!(
            fast_path_in_the_kernel(
                "ifNil: nilBlock ifNotNil: notNilBlock = ( ^notNilBlock cull: self )"
            ),
            Some(MethodFastPath::CullParameter(1))
        );
    }

    /// `cull:` with anything but `self`, or sent to anything but a
    /// parameter, is an ordinary method.
    #[test]
    fn does_not_recognize_another_cull() {
        assert_eq!(
            fast_path_in_the_kernel("ifNotNil: block = ( ^block cull: 3 )"),
            None
        );
        assert_eq!(
            fast_path_in_the_kernel("ifNotNil: block = ( ^block cull: block )"),
            None
        );
        assert_eq!(
            fast_path_in_the_kernel("ifNotNil: block = ( ^vmMirror cull: self )"),
            None
        );
        assert_eq!(
            fast_path_in_the_kernel("ifNotNil: block = ( ^block value: self )"),
            None
        );
    }

    #[test]
    fn does_not_recognize_a_receiver_or_cull_answer_outside_the_kernel() {
        assert_eq!(fast_path_outside_the_kernel("yourself = ( ^self )"), None);
        assert_eq!(
            fast_path_outside_the_kernel("ifNotNil: block = ( ^block cull: self )"),
            None
        );
    }
}
