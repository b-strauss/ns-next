//! Resolver entry point: `Module` -> `ResolvedModule`. See the runtime
//! spec's Resolver section for the algorithm this transcribes.

mod classes;
mod scope;
mod sends;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use ns_syntax::ast::{self, Expr, Stmt};
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
    let mut r = Resolver {
        bindings: HashMap::new(),
        errors: Vec::new(),
    };
    let member_sets = build_member_sets(&module.root_class);
    r.resolve_class(&module.root_class, &[], &member_sets);
    if r.errors.is_empty() {
        Ok(ResolvedModule {
            module: Rc::new(module),
            source,
            origin,
            bindings: Rc::new(r.bindings),
        })
    } else {
        Err(r.errors)
    }
}

/// Own + statically-inherited member names, per class declaration identity
/// (keyed by the class's own name, which assumes no module declares two
/// same-named classes at different nesting depths that would collide
/// within one compile; revisit with a real `ClassId` if that ever bites).
fn build_member_sets(root: &ast::ClassDecl) -> HashMap<Symbol, HashSet<Symbol>> {
    let mut sets = HashMap::new();
    collect_member_sets(root, &mut sets);
    sets
}

fn collect_member_sets(decl: &ast::ClassDecl, sets: &mut HashMap<Symbol, HashSet<Symbol>>) {
    let own = classes::own_member_names(decl);
    let superclass_name = decl
        .superclass
        .as_ref()
        .map(|clause| Symbol::intern(&clause.receiver.name))
        .unwrap_or(Symbol::OBJECT);
    // Inherit the superclass's member set if we've already computed it
    // (nested classes are declared textually after their enclosing class,
    // and Object/builtins are the fixed base case -- their own instance
    // side selectors are added by the bootstrap, not resolved
    // here). Missing/builtin superclasses contribute an empty set here;
    // that's fine because `depth_search` only needs *this* module's own
    // lexically declared names to pick a depth.
    let mut own_plus_inherited = own;
    if let Some(inherited) = sets.get(&superclass_name) {
        own_plus_inherited.extend(inherited.iter().copied());
    }
    let name = Symbol::intern(&decl.name.name);
    sets.insert(name, own_plus_inherited);
    for nested in &decl.instance_side.nested_classes {
        collect_member_sets(nested, sets);
    }
    if let Some(class_side) = &decl.class_side {
        for nested in &class_side.nested_classes {
            collect_member_sets(nested, sets);
        }
    }
}

/// Builds an innermost-first `EnclosingClass` chain from `names`
/// (outer-to-inner order, as accumulated while walking down nested class
/// declarations).
fn class_chain_from<'a>(
    names: &[Symbol],
    member_sets: &'a HashMap<Symbol, HashSet<Symbol>>,
) -> Vec<EnclosingClass<'a>> {
    names
        .iter()
        .rev()
        .map(|&n| EnclosingClass {
            name: n,
            members: member_sets.get(&n).unwrap_or_else(|| {
                static EMPTY: std::sync::OnceLock<HashSet<Symbol>> = std::sync::OnceLock::new();
                EMPTY.get_or_init(HashSet::new)
            }),
        })
        .collect()
}

struct Resolver {
    bindings: HashMap<Span, Binding>,
    errors: Vec<ResolveError>,
}

impl Resolver {
    fn resolve_class(
        &mut self,
        decl: &ast::ClassDecl,
        enclosing: &[Symbol],
        member_sets: &HashMap<Symbol, HashSet<Symbol>>,
    ) {
        let name = Symbol::intern(&decl.name.name);
        let mut chain: Vec<Symbol> = enclosing.to_vec();
        chain.push(name);
        // Innermost first for the chain we hand to `sends::depth_search`.
        let class_chain = class_chain_from(&chain, member_sets);

        // The superclass clause's receiver (e.g. `Object` in `Object new`)
        // is evaluated "as if it occurred inside an instance method of the
        // enclosing class" (`newspeak-spec.pdf` §6.1; `docs/design/dispatch.md`,
        // "Binding variants") -- so it is resolved against the
        // chain of classes *enclosing* this declaration, not against this
        // class's own not-yet-existing member set.
        if let Some(clause) = &decl.superclass {
            let enclosing_chain = class_chain_from(enclosing, member_sets);
            self.resolve_superclass_clause(clause, &enclosing_chain);
        }

        for method in &decl.instance_side.methods {
            self.resolve_method(method, &class_chain);
        }
        if let Some(class_side) = &decl.class_side {
            for method in &class_side.methods {
                self.resolve_method(method, &class_chain);
            }
        }
        // Slot initializers and initExprs execute in the instance
        // initializer's activation (`docs/design/object-model.md`,
        // "Instantiation", step 2: that activation is created with the
        // factory message's arguments), so this class's *own* factory parameters are
        // locals there -- exactly the way a method's own parameters are
        // locals in its body. `| t = platform transcript. |` inside
        // `class M usingPlatform: platform = Object new (...)` is the
        // single most common real Newspeak module idiom and depends on it.
        //
        // A class declaration with no explicit factory pattern
        // (`class C = Super (...)`) contributes no parameters, matching
        // `interpreter/instantiate.rs`'s `build_mixin`, which defaults such
        // a class's mixin `factory` to a bare unary `new` pattern.
        // Parameter order here is the same order `LocalScope::new` indexes
        // by, which is the order `Interpreter::instantiate` fills
        // `Frame.args` with its `factory_args` -- so `Binding::Local(i)`
        // reads the right argument at runtime.
        let factory_params: Vec<Symbol> = decl
            .factory
            .as_ref()
            .map(pattern_param_names)
            .unwrap_or_default();
        let mut scope = ScopeStack::new();
        scope.push(LocalScope::new(&factory_params));
        for slot in &decl.instance_side.slots {
            if let Some(init) = &slot.init {
                self.resolve_expr(init, &mut scope, &class_chain);
            }
        }
        for stmt in &decl.instance_side.init_exprs {
            self.resolve_stmt(stmt, &mut scope, &class_chain);
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
        if let Some(clause) = &decl.superclass {
            if let Some(message) = crate::interpreter::nested::superclass_clause_message(clause) {
                for arg in &message.args {
                    self.resolve_expr(arg, &mut scope, &class_chain);
                }
            }
        }

        for nested in &decl.instance_side.nested_classes {
            self.resolve_class(nested, &chain, member_sets);
        }
        if let Some(class_side) = &decl.class_side {
            for nested in &class_side.nested_classes {
                self.resolve_class(nested, &chain, member_sets);
            }
        }
    }

    fn resolve_method(&mut self, method: &ast::MethodDecl, class_chain: &[EnclosingClass]) {
        let param_names: Vec<Symbol> = pattern_param_names(&method.pattern);
        let temp_names: Vec<Symbol> = method
            .body
            .temps
            .iter()
            .map(|t| Symbol::intern(&t.name))
            .collect();
        let mut all = param_names;
        all.extend(temp_names);
        let mut scope = ScopeStack::new();
        scope.push(LocalScope::new(&all));
        for stmt in &method.body.stmts {
            self.resolve_stmt(stmt, &mut scope, class_chain);
        }
    }

    fn resolve_stmt(&mut self, stmt: &Stmt, scope: &mut ScopeStack, chain: &[EnclosingClass]) {
        match stmt {
            Stmt::Expr(e) | Stmt::Return(e) => self.resolve_expr(e, scope, chain),
        }
    }

    fn resolve_expr(&mut self, expr: &Expr, scope: &mut ScopeStack, chain: &[EnclosingClass]) {
        match expr {
            Expr::Ident(id) => self.resolve_ident(id, scope, chain),
            Expr::Literal(_) => {}
            Expr::Send(send) => self.resolve_send(send, scope, chain),
            Expr::Cascade(cascade) => {
                self.resolve_expr(&cascade.receiver, scope, chain);
                self.resolve_message_send(&cascade.first, scope, chain);
                for m in &cascade.rest {
                    self.resolve_message_send(m, scope, chain);
                }
            }
            Expr::Assign { target, value, .. } => {
                self.resolve_setter_target(target, scope, chain);
                self.resolve_expr(value, scope, chain);
            }
            Expr::Block(block) => {
                let param_names: Vec<Symbol> = block
                    .params
                    .iter()
                    .map(|p| Symbol::intern(&p.name))
                    .collect();
                let temp_names: Vec<Symbol> = block
                    .body
                    .temps
                    .iter()
                    .map(|t| Symbol::intern(&t.name))
                    .collect();
                let mut names = param_names;
                names.extend(temp_names);
                scope.push(LocalScope::new(&names));
                for stmt in &block.body.stmts {
                    self.resolve_stmt(stmt, scope, chain);
                }
                scope.pop();
            }
            Expr::DynamicArray { elements, .. } => {
                for e in elements {
                    self.resolve_expr(e, scope, chain);
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
        for arg in &message.args {
            self.resolve_expr(arg, scope, chain);
        }
    }

    /// A bare `Expr::Ident` standing alone (not the receiver of an
    /// enclosing `Send`, since that case is handled in `resolve_send`
    /// below): try locals first, then the §5.7 depth search treating the
    /// name itself as a zero-arg implicit send. `self`/`super` alone (no
    /// message) need no binding entry -- the interpreter special-cases
    /// them directly against the current `Frame`.
    ///
    /// Mirrors `resolve_superclass_clause`'s `None` prefix case: a bare
    /// name that genuinely resolves nowhere lexically (`find_member_depth`
    /// returns `None`, unlike `depth_search`'s always-defaults-to-0
    /// behavior) is checked against `classes::is_builtin_class_name`
    /// *before* falling back to a self send. A builtin match records no
    /// `Binding` at all for this span -- the compiler resolves it against
    /// the builtin table by name instead (`Instr::LoadBuiltinClass`).
    fn resolve_ident(&mut self, id: &ast::Ident, scope: &mut ScopeStack, chain: &[EnclosingClass]) {
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
        // actual runtime class).
        self.resolve_expr(&send.receiver, scope, chain);
        self.resolve_message_send(&send.message, scope, chain);
    }

    /// The `id` in `id:: e` (`Expr::Assign { target, .. }`) resolves
    /// exactly like an implicit-receiver keyword send to `id:` -- see
    /// `docs/design/dispatch.md`, "Setter sends". Locals win first (a local
    /// temp/param can be reassigned directly); otherwise it is a
    /// `LexicalSend` at the depth found for the plain slot name (the
    /// setter and getter share one declared name -- see
    /// `classes::member_name_of_pattern`).
    fn resolve_setter_target(
        &mut self,
        target: &ast::Ident,
        scope: &mut ScopeStack,
        chain: &[EnclosingClass],
    ) {
        let name = Symbol::intern(&target.name);
        if let Some(binding) = scope.resolve(name) {
            self.bindings.insert(target.span, binding);
            return;
        }
        let depth = sends::depth_search(chain, name);
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

fn pattern_param_names(pattern: &ast::MessagePattern) -> Vec<Symbol> {
    match pattern {
        ast::MessagePattern::Unary { .. } => vec![],
        ast::MessagePattern::Binary { param, .. } => vec![Symbol::intern(&param.name)],
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|(_, param)| Symbol::intern(&param.name))
            .collect(),
    }
}

fn member_name_of_message(message: &ast::MessageSend) -> Symbol {
    match &message.selector {
        ast::Selector::Unary(name) => Symbol::intern(name),
        ast::Selector::Binary(name) => Symbol::intern(name),
        ast::Selector::Keyword(full) => {
            // `full` is the whole selector ("at:put:"); §5.7 looks a
            // keyword send up under its first keyword ("at").
            let first = full.split(':').next().unwrap_or(full.as_str());
            Symbol::intern(first)
        }
    }
}
