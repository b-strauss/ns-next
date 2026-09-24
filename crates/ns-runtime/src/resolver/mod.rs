//! Resolver entry point: `Module` -> `ResolvedModule`. See the runtime
//! spec's Resolver section for the algorithm this transcribes.

mod classes;
mod scope;
pub(crate) mod sends;

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast::{self, Expression, Statement};
use ns_syntax::Span;

use crate::types::{Binding, ResolveError, ResolvedModule, SourceOrigin};
use crate::Symbol;

use scope::{LocalScope, ScopeStack};
use sends::EnclosingClass;

pub fn resolve(
    module: ast::Module,
    source: Rc<str>,
    origin: SourceOrigin,
) -> Result<ResolvedModule, Vec<ResolveError>> {
    resolve_with(module, source, origin, false)
}

/// `resolve`, for `Kernel.ns` alone: additionally marks each send to the
/// root class's `vmMirror` with `Binding::MirrorSend`. Only the bootstrap
/// (`interpreter::kernel`) and crate-internal tests call it.
pub(crate) fn resolve_kernel(
    module: ast::Module,
    source: Rc<str>,
    origin: SourceOrigin,
) -> Result<ResolvedModule, Vec<ResolveError>> {
    resolve_with(module, source, origin, true)
}

/// The one body of `resolve` and `resolve_kernel`: `kernel` is the only
/// thing that differs, so a user module and the kernel can never be resolved
/// by two drifting copies of the walk.
fn resolve_with(
    module: ast::Module,
    source: Rc<str>,
    origin: SourceOrigin,
    kernel: bool,
) -> Result<ResolvedModule, Vec<ResolveError>> {
    let mut resolver = Resolver {
        bindings: HashMap::new(),
        errors: Vec::new(),
        kernel,
    };
    resolver.resolve_class(&module.root_class, &[]);
    if resolver.errors.is_empty() {
        Ok(ResolvedModule {
            module: Rc::new(module),
            source,
            origin,
            bindings: Rc::new(resolver.bindings),
            kernel,
        })
    } else {
        Err(resolver.errors)
    }
}

struct Resolver {
    bindings: HashMap<Span, Binding>,
    errors: Vec<ResolveError>,
    /// Whether this is `Kernel.ns`, whose sends to `vmMirror` are marked
    /// (`resolve_kernel`). `false` for every user module.
    kernel: bool,
}

impl Resolver {
    /// `enclosing` is the chain of declarations around this one, innermost
    /// first. Each level borrows the member set of *its own* declaration,
    /// computed here as the walk enters it, so two classes of one simple name
    /// can never share or overwrite a set (`docs/design/dispatch.md`,
    /// "Resolution"). Nothing inherited goes in: §5.7 looks for the innermost
    /// lexically enclosing construct in which a member "is declared", and
    /// §2.3.1 proceeds "up the inheritance chain of self" only once no
    /// lexically visible declaration matches -- dispatch's job at run time.
    fn resolve_class(
        &mut self,
        declaration: &ast::ClassDeclaration,
        enclosing: &[EnclosingClass<'_>],
    ) {
        let name = Symbol::intern(&declaration.name.name);
        for duplicate in classes::duplicate_member_names(declaration) {
            self.errors.push(ResolveError {
                span: declaration.name.span,
                message: format!("class `{name}` has multiple definitions of `{duplicate}`"),
            });
        }
        let own_members = classes::own_member_names(declaration);
        let class_chain: Vec<EnclosingClass<'_>> = std::iter::once(EnclosingClass {
            name,
            members: &own_members,
        })
        .chain(enclosing.iter().copied())
        .collect();

        // A module root's clauses are decided by the root rule, deferred to
        // instantiation (`interpreter::instantiate::root_clause_defect`), so
        // they bind nothing here: a receiver with no binding lowers to
        // `LoadBuiltinClass` or a deferred `UnresolvedIdentifier`, and a
        // root's clause receiver is never run. Refusing `Missing` here while
        // `Array` ran was one rule on two paths.
        if !enclosing.is_empty() {
            // The superclass clause's receiver (e.g. `Object` in `Object new`)
            // is evaluated "as if it occurred inside an instance method of the
            // enclosing class" (`newspeak-spec.pdf` §6.1; `docs/design/dispatch.md`,
            // "Binding variants") -- so it is resolved against the
            // chain of classes *enclosing* this declaration, not against this
            // class's own not-yet-existing member set.
            if let Some(clause) = &declaration.superclass {
                self.resolve_superclass_clause(clause, enclosing);
            }
            // §6.2 builds a mixin application out of §6.1's very `inheritanceClause`
            // production -- "followed by the mixin application operator `<:`,
            // followed by an inheritance clause" -- so each `<:` clause's receiver
            // is evaluated under the same rule as the superclass clause's, in the
            // same enclosing scope. That is why `resolve_superclass_clause` is
            // called for them rather than a second copy of the rule being written
            // here. Their deferred *message* arguments are resolved below, beside
            // the superclass clause's, for the same reason.
            for clause in &declaration.mixin_applications {
                self.resolve_superclass_clause(clause, enclosing);
            }
        }

        for method in &declaration.instance_side.methods {
            self.resolve_method(method, &class_chain);
        }
        if let Some(class_side) = &declaration.class_side {
            for method in &class_side.methods {
                self.resolve_method(method, &class_chain);
            }
        }
        // Slot initializers and initExprs execute in the instance
        // initializer's activation (`docs/design/object-model.md`,
        // "Instantiation", step 2: that activation is created with the
        // factory message's arguments), so this class's *own* factory
        // parameters are locals there -- exactly the way a method's own
        // parameters are locals in its body. `| t = platform transcript. |` inside
        // `class M usingPlatform: platform = Object new (...)` is the
        // single most common real Newspeak module idiom and depends on it.
        //
        // A class declaration with no explicit factory pattern (`class C =
        // Super (...)`) contributes no parameters, matching
        // `interpreter/instantiate.rs`'s `build_mixin`, which defaults such a
        // class's mixin `factory` to a bare unary `new` pattern. Parameter
        // order here is the same order `LocalScope::new` indexes by, which is
        // the order `Interpreter::instantiate` fills `Frame.arguments` with its
        // `factory_arguments` -- so `Binding::Local(i)` reads the right
        // argument at runtime.
        let factory_parameters: Vec<Symbol> = declaration
            .factory
            .as_ref()
            .map(pattern_parameter_names)
            .unwrap_or_default();
        let mut scope = ScopeStack::new();
        scope.push(LocalScope::new(&factory_parameters, &[]));
        for slot in &declaration.instance_side.slots {
            if let Some(initializer) = &slot.initializer {
                self.resolve_expression(initializer, &mut scope, &class_chain);
            }
        }
        for statement in &declaration.instance_side.initializer_expressions {
            self.resolve_statement(statement, &mut scope, &class_chain);
        }
        // The superclass clause's *deferred message* arguments (`m + 1` in
        // `= Base withBase: m + 1`) run in that same activation:
        // `newspeak-spec.pdf` §6.2 evaluates the clause's message on every
        // instantiation, in the instance initializer's own context, so this
        // class's factory parameters are in scope for them exactly as they
        // are for a slot initializer. The clause's *receiver* is resolved
        // separately, above, against the enclosing chain instead -- it is
        // evaluated once, when the declaration is evaluated, and never sees
        // a factory argument.
        //
        // §6.2 evaluates each `<:` clause's message in that same instance
        // initializer, after the superclass clause's, so its arguments see
        // the same factory parameters.
        for clause in declaration
            .superclass
            .iter()
            .chain(&declaration.mixin_applications)
        {
            if let Some(message) = crate::interpreter::nested::superclass_clause_message(clause) {
                for argument in &message.arguments {
                    self.resolve_expression(argument, &mut scope, &class_chain);
                }
            }
        }

        for nested in &declaration.instance_side.nested_classes {
            self.resolve_class(nested, &class_chain);
        }
        if let Some(class_side) = &declaration.class_side {
            for nested in &class_side.nested_classes {
                self.resolve_class(nested, &class_chain);
            }
        }
    }

    fn resolve_method(&mut self, method: &ast::MethodDeclaration, class_chain: &[EnclosingClass]) {
        let parameter_names: Vec<Symbol> = pattern_parameter_names(&method.pattern);
        let mut scope = ScopeStack::new();
        scope.push(LocalScope::new(&parameter_names, &method.body.temporaries));
        self.resolve_local_initializers(&method.body.temporaries, &mut scope, class_chain);
        for statement in &method.body.statements {
            self.resolve_statement(statement, &mut scope, class_chain);
        }
    }

    /// A local slot's initializer runs in the activation it belongs to
    /// (§3.6), so it resolves in the scope its body's statements do.
    fn resolve_local_initializers(
        &mut self,
        temporaries: &[ast::SlotDeclaration],
        scope: &mut ScopeStack,
        chain: &[EnclosingClass],
    ) {
        for temporary in temporaries {
            if let Some(initializer) = &temporary.initializer {
                self.resolve_expression(initializer, scope, chain);
            }
        }
    }

    fn resolve_statement(
        &mut self,
        statement: &Statement,
        scope: &mut ScopeStack,
        chain: &[EnclosingClass],
    ) {
        match statement {
            Statement::Expression(expression) | Statement::Return(expression) => {
                self.resolve_expression(expression, scope, chain)
            }
        }
    }

    fn resolve_expression(
        &mut self,
        expression: &Expression,
        scope: &mut ScopeStack,
        chain: &[EnclosingClass],
    ) {
        match expression {
            Expression::Identifier(id) => self.resolve_identifier(id, scope, chain),
            Expression::Literal(_) => {}
            Expression::Send(send) => self.resolve_send(send, scope, chain),
            Expression::Cascade(cascade) => {
                self.resolve_expression(&cascade.receiver, scope, chain);
                self.resolve_message_send(&cascade.first, scope, chain);
                for message in &cascade.rest {
                    self.resolve_message_send(message, scope, chain);
                }
            }
            Expression::Assign { target, value, .. } => {
                self.resolve_setter_target(target, scope, chain);
                self.resolve_expression(value, scope, chain);
            }
            Expression::Block(block) => {
                let parameter_names: Vec<Symbol> = block
                    .parameters
                    .iter()
                    .map(|parameter| Symbol::intern(&parameter.name))
                    .collect();
                scope.push(LocalScope::new(&parameter_names, &block.body.temporaries));
                self.resolve_local_initializers(&block.body.temporaries, scope, chain);
                for statement in &block.body.statements {
                    self.resolve_statement(statement, scope, chain);
                }
                scope.pop();
            }
            Expression::DynamicArray { elements, .. } => {
                for element in elements {
                    self.resolve_expression(element, scope, chain);
                }
            }
        }
    }

    fn resolve_message_send(
        &mut self,
        message: &ast::MessageSend,
        scope: &mut ScopeStack,
        chain: &[EnclosingClass],
    ) {
        for argument in &message.arguments {
            self.resolve_expression(argument, scope, chain);
        }
    }

    /// A bare `Expression::Identifier` standing alone (not the receiver of an
    /// enclosing `Send`, since that case is handled in `resolve_send`
    /// below): try locals first, then the §5.7 depth search treating the
    /// name itself as a zero-argument implicit send. `self`/`super` alone (no
    /// message) need no binding entry -- the interpreter special-cases
    /// them directly against the current `Frame`.
    ///
    /// Mirrors `resolve_superclass_clause`'s `None` prefix case: a bare
    /// name that genuinely resolves nowhere lexically (`find_member_depth`
    /// returns `None`, unlike `depth_search`'s always-defaults-to-0
    /// behavior) is checked against `classes::is_builtin_class_name`
    /// *before* falling back to a self send. A builtin match records no
    /// `Binding` at all for this span -- the compiler resolves it against
    /// the builtin table by name instead (`Instruction::LoadBuiltinClass`).
    fn resolve_identifier(
        &mut self,
        id: &ast::Identifier,
        scope: &mut ScopeStack,
        chain: &[EnclosingClass],
    ) {
        if id.name == "self" || id.name == "super" {
            return;
        }
        let name = Symbol::intern(&id.name);
        if let Some(binding) = scope.resolve(name) {
            self.bindings.insert(id.span, binding);
            return;
        }
        match sends::find_member_depth(chain, name) {
            Some(depth) => {
                self.bindings
                    .insert(id.span, Binding::LexicalSend { depth });
            }
            None if classes::is_builtin_class_name(name) => {
                // No lexical declaration and none needed -- the base case
                // that lets a bare builtin-class-name reference (e.g.
                // `OrderedCollection new` outside any superclass clause)
                // resolve against the builtin table instead of becoming a
                // depth-0 self send.
            }
            None => {
                // Not found at any lexical depth and not a builtin --
                // per the spec, an implicit send that matches nothing
                // falls back to a depth-0 self send (dispatch then
                // naturally produces `doesNotUnderstand:`).
                self.bindings
                    .insert(id.span, Binding::LexicalSend { depth: 0 });
            }
        }
    }

    fn resolve_send(&mut self, send: &ast::Send, scope: &mut ScopeStack, chain: &[EnclosingClass]) {
        if sends::is_explicit_super(&send.receiver) {
            self.bindings.insert(send.span, Binding::SuperSend);
            self.resolve_message_send(&send.message, scope, chain);
            return;
        }
        if sends::is_explicit_self(&send.receiver) {
            self.bindings
                .insert(send.span, Binding::LexicalSend { depth: 0 });
            self.resolve_message_send(&send.message, scope, chain);
            return;
        }
        if sends::is_synthesized_implicit_receiver(&send.receiver) {
            let selector_name = member_name_of_message(&send.message);
            let depth = sends::depth_search(chain, selector_name);
            self.bindings
                .insert(send.span, Binding::LexicalSend { depth });
            self.resolve_message_send(&send.message, scope, chain);
            return;
        }
        if let Some(class_name) = sends::outer_class_name(&send.receiver) {
            let name = Symbol::intern(class_name);
            match sends::depth_of_named_outer(chain, name) {
                Some(depth) => {
                    self.bindings
                        .insert(send.span, Binding::LexicalSend { depth });
                }
                None => self.errors.push(ResolveError {
                    span: send.span,
                    message: format!(
                        "`outer {class_name}` does not name a lexically enclosing class"
                    ),
                }),
            }
            self.resolve_message_send(&send.message, scope, chain);
            return;
        }
        // Ordinary explicit-receiver send: resolve the receiver
        // sub-expression on its own merits; the send itself needs no
        // binding (dispatch.rs's lookupPublic runs on the receiver's
        // actual runtime class). In the kernel, a send to `vmMirror` is
        // marked for the recognizers that inline a mirror send.
        if self.kernel && is_the_mirror_slot(&send.receiver, scope, chain) {
            self.bindings.insert(send.span, Binding::MirrorSend);
        }
        self.resolve_expression(&send.receiver, scope, chain);
        self.resolve_message_send(&send.message, scope, chain);
    }

    /// The `id` in `id:: e` (`Expression::Assign { target, .. }`) resolves
    /// exactly like an implicit-receiver keyword send to `id:` -- see
    /// `docs/design/dispatch.md`, "Setter sends", and `newspeak-spec.pdf`
    /// §5.4.3, which makes `id:: e` the send of `id:`. Only a local that
    /// declares a setter wins first -- a parameter or an immutable local
    /// declares no `id:`, so it is passed by, as the canonical compiler
    /// does. Otherwise it is a `LexicalSend` at the depth of the innermost
    /// declaration of the setter `id:` itself (§5.7) -- a mutable slot `id`,
    /// or a method `id:`.
    fn resolve_setter_target(
        &mut self,
        target: &ast::Identifier,
        scope: &mut ScopeStack,
        chain: &[EnclosingClass],
    ) {
        let name = Symbol::intern(&target.name);
        if let Some(binding) = scope.resolve_setter(name) {
            self.bindings.insert(target.span, binding);
            return;
        }
        let setter = Symbol::intern(&format!("{}:", target.name));
        let depth = sends::depth_search(chain, setter);
        self.bindings
            .insert(target.span, Binding::LexicalSend { depth });
    }

    /// Resolves a superclass clause's `receiver` (e.g. `Object` in
    /// `Object new`) -- see `docs/design/dispatch.md`, "Binding variants".
    /// `prefix` is handled the same way an explicit
    /// self/super/outer receiver would be in an ordinary send; a bare
    /// receiver is looked up as a member name (a nested class is referenced
    /// through its auto-generated accessor, so it is already present in
    /// `own_member_names`) against `enclosing_chain`. Builtin class names
    /// (`Object` and the rest of the core library) have no
    /// lexical declaration to find -- that is the base case that lets a
    /// class hierarchy bottom out, so only a name that is *neither* found
    /// lexically *nor* a known builtin is a `ResolveError`.
    fn resolve_superclass_clause(
        &mut self,
        clause: &ast::SuperclassClause,
        enclosing_chain: &[EnclosingClass],
    ) {
        match &clause.prefix {
            Some(ast::SuperclassPrefix::SelfPrefix) => {
                self.bindings
                    .insert(clause.receiver.span, Binding::LexicalSend { depth: 0 });
            }
            Some(ast::SuperclassPrefix::SuperPrefix) => {
                self.bindings
                    .insert(clause.receiver.span, Binding::SuperSend);
            }
            Some(ast::SuperclassPrefix::Outer(outer_id)) => {
                let name = Symbol::intern(&outer_id.name);
                match sends::depth_of_named_outer(enclosing_chain, name) {
                    Some(depth) => {
                        self.bindings
                            .insert(clause.receiver.span, Binding::LexicalSend { depth });
                    }
                    None => self.errors.push(ResolveError {
                        span: clause.span,
                        message: format!(
                            "`outer {}` does not name a lexically enclosing class",
                            outer_id.name
                        ),
                    }),
                }
            }
            None => {
                let name = Symbol::intern(&clause.receiver.name);
                match sends::find_member_depth(enclosing_chain, name) {
                    Some(depth) => {
                        self.bindings
                            .insert(clause.receiver.span, Binding::LexicalSend { depth });
                    }
                    None if classes::is_builtin_class_name(name) => {
                        // No lexical declaration and none needed -- this is
                        // the base case that lets the hierarchy bottom out
                        // at a core-library class.
                    }
                    // §6.1 evaluates the clause "as if it occurred inside an
                    // instance method of E", the enclosing class, where a
                    // name no enclosing declaration declares is an implicit
                    // receiver send "equivalent to a self send" (§5.7) --
                    // a nested class the enclosing class only inherits is
                    // found that way, at run time.
                    None if !enclosing_chain.is_empty() => {
                        self.bindings
                            .insert(clause.receiver.span, Binding::LexicalSend { depth: 0 });
                    }
                    // At the top level there is no enclosing object to send
                    // to: §3.8, "a top level expression is not evaluated in
                    // the context of an enclosing object; this precludes
                    // implicit receiver sends".
                    None => self.errors.push(ResolveError {
                        span: clause.receiver.span,
                        message: format!(
                            "`{}` does not name an enclosing class member or a known builtin",
                            clause.receiver.name
                        ),
                    }),
                }
            }
        }
    }
}

/// The bare identifier `vmMirror`, not a local, naming a member of the
/// module's root class -- the outermost entry of `chain`. That it is an
/// immutable slot holding the mirror is the bootstrap's to check
/// (`interpreter::kernel`), since the resolver cannot know what a slot holds.
fn is_the_mirror_slot(receiver: &Expression, scope: &ScopeStack, chain: &[EnclosingClass]) -> bool {
    let Expression::Identifier(identifier) = receiver else {
        return false;
    };
    if identifier.name != "vmMirror" {
        return false;
    }
    let name = Symbol::intern(&identifier.name);
    scope.resolve(name).is_none()
        && sends::find_member_depth(chain, name)
            .is_some_and(|depth| usize::from(depth) + 1 == chain.len())
}

fn pattern_parameter_names(pattern: &ast::MessagePattern) -> Vec<Symbol> {
    match pattern {
        ast::MessagePattern::Unary { .. } => vec![],
        ast::MessagePattern::Binary { parameter, .. } => vec![Symbol::intern(&parameter.name)],
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|(_, parameter)| Symbol::intern(&parameter.name))
            .collect(),
    }
}

/// The member an implicit-receiver send names: `newspeak-spec.pdf` §5.7's
/// "Let s be the selector of the message", the whole selector -- `at:put:`
/// for a keyword send, never its first keyword.
fn member_name_of_message(message: &ast::MessageSend) -> Symbol {
    match &message.selector {
        ast::Selector::Unary(name) | ast::Selector::Binary(name) | ast::Selector::Keyword(name) => {
            Symbol::intern(name)
        }
    }
}
