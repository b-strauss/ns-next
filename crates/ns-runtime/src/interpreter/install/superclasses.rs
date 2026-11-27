//! A superclass change: an install whose edits give a declaration another
//! superclass clause or other `<:` clauses evaluates the new clauses for
//! every class object of the declaration, as program code against the
//! unchanged world, between the leaf that prepares the install and the
//! leaf that commits it (`clause_evaluation`); the commit checks what they
//! answered -- every class object evaluated, each answer a class, no
//! cycle -- before it replaces anything (`clause_reparentings`,
//! `refuse_a_cycle`), and re-applies each class object over its answer
//! (`Mutator::reapply`), running no code. A module's root evaluates
//! nothing: its superclass follows its fresh `root_shape`
//! (`root_reparentings`). `docs/design/liveness-and-reflection.md`,
//! "Planned: the complete install", decision 1.

use super::prepared::PreparedInstall;
use crate::hash::IdentityMap;
use crate::heap::{ClassReference, MixinId, ObjectReference};
use crate::interpreter::edit::InstallEntry;
use crate::interpreter::{Mutator, Step};
use std::rc::Rc;

use crate::mixin::{Mixin, MixinApplication};
use crate::types::{BuiltinClass, Value};

/// A class object the commit re-parents, and what over.
pub(super) struct Reparenting<'gc> {
    /// The class object, which keeps its identity.
    pub(super) class: ClassReference<'gc>,
    /// The superclass below every `<:` level: what the new clause
    /// answered, or for a root the builtin its fresh `root_shape` names.
    pub(super) superclass: ClassReference<'gc>,
    /// Each `<:` clause's index with the class its receiver answered, in
    /// clause order, from which the commit builds the levels anew
    /// (`nested::build_applied_levels`).
    pub(super) applied: Vec<(usize, Value<'gc>)>,
    /// For a body-less declaration's class object, the declaration's id:
    /// the class object is the level its last clause adds, so the commit
    /// builds the levels below it anew and makes the class object itself
    /// the level of the new last clause.
    pub(super) body_less: Option<MixinId>,
}

impl<'gc> Mutator<'gc> {
    /// The evaluation leaf's step for `class`, one of the class objects
    /// the preparation of `entries` at `epoch` answered: the activation of
    /// its declaration's fresh superclass clause on its enclosing object,
    /// as part of the class that declares it, which delivers its value and
    /// caches nothing; `Object` itself for a declaration that now writes
    /// no clause. `None` for a moved epoch and for any other object.
    pub(crate) fn clause_evaluation(
        &mut self,
        entries: &[InstallEntry],
        class: ClassReference<'gc>,
        epoch: u64,
    ) -> Option<Step<'gc>> {
        if epoch != self.install_epoch {
            return None;
        }
        let prepared = self.prepare_install(entries).ok()?;
        let id = self.clause_declaration(&prepared, class)?;
        let fresh = prepared.fresh_version(id)?;
        let Some(code) = fresh.superclass_clause_code.clone() else {
            let object = self.builtin_class(BuiltinClass::Object);
            return Some(Step::Value(Value::object(object)));
        };
        let (enclosing, declaring_class) = self.clause_context(class)?;
        Some(Step::ClauseEvaluation {
            code,
            enclosing,
            declaring_class,
        })
    }

    /// The enclosing object and the declaring class `class`'s declaration
    /// was evaluated in, which its clause runs in: a body-less
    /// declaration's class object keeps them as its declaration scope,
    /// since its own are the named class's (§6.2).
    pub(crate) fn clause_context(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<(ObjectReference<'gc>, ClassReference<'gc>)> {
        match self.heap.class_declaration_scope(class) {
            Some(scope) => Some((scope.enclosing?, scope.declaring_class?)),
            None => Some((
                self.heap.class_enclosing(class)?,
                self.heap.class_declaring(class)?,
            )),
        }
    }

    /// Whether `answers`, the commit leaf's pairs, each name a distinct
    /// class object the preparation of `entries` evaluates a clause for:
    /// what the shipped installer hands over, so anything else is
    /// declined. A moved epoch or a refused preparation passes, since the
    /// commit refuses it with its diagnostic.
    pub(crate) fn answers_name_clause_classes(
        &mut self,
        entries: &[InstallEntry],
        epoch: u64,
        answers: &[(ClassReference<'gc>, Value<'gc>)],
    ) -> bool {
        if epoch != self.install_epoch {
            return true;
        }
        let Ok(prepared) = self.prepare_install(entries) else {
            return true;
        };
        // Unreachable from the shipped installer while the epoch holds: it
        // hands over exactly the class objects the preparation answered,
        // each once.
        let mut seen: IdentityMap<usize, ()> = IdentityMap::default();
        answers.iter().all(|(class, _)| {
            self.clause_declaration(&prepared, *class).is_some()
                && seen
                    .insert(self.heap.object_identity_key(*class), ())
                    .is_none()
        })
    }

    /// Every class object the commit of `prepared` re-parents over what
    /// its new clause answered, by `answers`, the evaluated class objects
    /// with their clauses' answers; or the diagnostic that refuses the
    /// install: a class object of a changed declaration that was not
    /// evaluated, since it was made while the clauses were, and an answer
    /// that is no class -- for `<:` clauses, no `{superclass. mixin…}` of
    /// classes, as the clause's body answers.
    pub(super) fn clause_reparentings(
        &self,
        prepared: &PreparedInstall,
        answers: &[(ClassReference<'gc>, Value<'gc>)],
    ) -> Result<Vec<Reparenting<'gc>>, String> {
        let evaluated: IdentityMap<usize, Value<'gc>> = answers
            .iter()
            .map(|(class, answer)| (self.heap.object_identity_key(*class), *answer))
            .collect();
        let mut reparentings = Vec::new();
        for class in self.clause_class_objects(prepared) {
            let id = self
                .clause_declaration(prepared, class)
                .expect("internal: the walk collects class objects of changed declarations");
            let name = self.declaration_name(id);
            let Some(answer) = evaluated.get(&self.heap.object_identity_key(class)) else {
                return Err(format!(
                    "a class object of {name} was made while its superclass clauses were \
                     evaluated; nothing was installed"
                ));
            };
            let fresh = prepared
                .fresh_version(id)
                .expect("internal: a changed declaration has a fresh version");
            let (superclass, applied) = self.unpack_answer(fresh, *answer, &name)?;
            reparentings.push(Reparenting {
                class,
                superclass,
                applied,
                body_less: (!fresh.has_body).then_some(id),
            });
        }
        Ok(reparentings)
    }

    /// The superclass and the `<:` clauses' classes `answer` gives a class
    /// object of `fresh`, or the diagnostic naming what is no class.
    fn unpack_answer(
        &self,
        fresh: &Mixin,
        answer: Value<'gc>,
        name: &str,
    ) -> Result<(ClassReference<'gc>, Vec<(usize, Value<'gc>)>), String> {
        let no_class = |value: Value<'gc>| {
            format!(
                "the superclass clause of {name} answered {}, which is no class",
                self.heap.describe(value)
            )
        };
        let class_of = |value: Value<'gc>| {
            value
                .as_object()
                .filter(|object| self.heap.class_application(*object).is_some())
                .ok_or_else(|| no_class(value))
        };
        if fresh.mixin_applications.is_empty() {
            return Ok((class_of(answer)?, Vec::new()));
        }
        let elements = answer
            .as_object()
            .and_then(|object| self.heap.array_elements(object))
            .filter(|elements| elements.len() == 1 + fresh.mixin_applications.len())
            .ok_or_else(|| no_class(answer))?;
        let superclass = class_of(elements[0])?;
        let mut applied = Vec::with_capacity(elements.len() - 1);
        for (index, element) in elements[1..].iter().enumerate() {
            class_of(*element)?;
            applied.push((index, *element));
        }
        Ok((superclass, applied))
    }

    /// Every root class object whose declaration's clauses `prepared`
    /// changes, re-parented over the builtin its fresh `root_shape` names,
    /// `Value` applied over it when the shape applies it, as
    /// `Mutator::root_class_of_mixin` builds a root. A walk of the space,
    /// only when a root's clauses change: a body-less root's class object
    /// applies `Value`'s mixin, so no walk by the replaced ids finds it.
    pub(super) fn root_reparentings(&self, prepared: &PreparedInstall) -> Vec<Reparenting<'gc>> {
        if !prepared
            .clauses_changed
            .iter()
            .any(|id| self.heap.enclosing_mixin_id(*id).is_none())
        {
            return Vec::new();
        }
        let mut classes = Vec::new();
        self.heap.each_object(|object| {
            if self.declaration_of_class_object(object).is_some() {
                classes.push(object);
            }
        });
        classes
            .iter()
            .filter_map(|class| {
                let id = self.declaration_of_class_object(*class)?;
                if !prepared.clauses_changed.contains(&id)
                    || self.heap.enclosing_mixin_id(id).is_some()
                {
                    return None;
                }
                let fresh = prepared.fresh_version(id)?;
                let shape = fresh.root_shape;
                let over_value = shape.is_some_and(|shape| shape.over_value);
                let applies_value = shape.is_some_and(|shape| shape.applies_value);
                let superclass = self.builtin_class(if over_value {
                    BuiltinClass::Value
                } else {
                    BuiltinClass::Object
                });
                let applied = if applies_value {
                    vec![(0, Value::object(self.builtin_class(BuiltinClass::Value)))]
                } else {
                    Vec::new()
                };
                Some(Reparenting {
                    class: *class,
                    superclass,
                    applied,
                    body_less: (!fresh.has_body).then_some(id),
                })
            })
            .collect()
    }

    /// Refuses `reparentings` when a class object's new superclass chain
    /// -- through the new superclass of every class object re-parented --
    /// reaches the class object itself, or one of its `<:` clauses names
    /// it: decided before anything is written, since every walk of the
    /// chain after the commit assumes it ends.
    pub(super) fn refuse_a_cycle(&self, reparentings: &[Reparenting<'gc>]) -> Result<(), String> {
        let new_superclass: IdentityMap<usize, ClassReference<'gc>> = reparentings
            .iter()
            .map(|reparenting| {
                (
                    self.heap.object_identity_key(reparenting.class),
                    reparenting.superclass,
                )
            })
            .collect();
        for reparenting in reparentings {
            let class = reparenting.class;
            let names_itself = reparenting
                .applied
                .iter()
                .any(|(_, applied)| applied.as_object().is_some_and(|object| object.same(class)));
            let mut visited: IdentityMap<usize, ()> = IdentityMap::default();
            let mut level = Some(reparenting.superclass);
            let mut cycle = names_itself;
            while let Some(current) = level
                && !cycle
            {
                if current.same(class) {
                    cycle = true;
                    break;
                }
                let key = self.heap.object_identity_key(current);
                // A loop that does not pass through `class` is found from
                // a class object on it.
                if visited.insert(key, ()).is_some() {
                    break;
                }
                level = match new_superclass.get(&key) {
                    Some(superclass) => Some(*superclass),
                    None => self.heap.class_superclass(current),
                };
            }
            if cycle {
                let name = self
                    .declaration_of_class_object(class)
                    .map_or_else(String::new, |id| self.declaration_name(id));
                return Err(format!(
                    "the superclass clause of {name} answers a class that inherits from {name}"
                ));
            }
        }
        Ok(())
    }

    /// Re-parents `class`, the class object of the body-less declaration
    /// `declaration` -- the level its last `<:` clause adds -- as
    /// `reparenting` says, over `superclass` and its application: the
    /// levels of every clause but the last built anew over it, then the
    /// class object itself made the level of the new last clause, keeping
    /// its identity and its declaration's scope: the applied class's
    /// mixin applied over the levels below, the clause, the class it names
    /// and that class's enclosing object and declaring class, which the
    /// applied mixin's code walks out from (§6.2). Answers the class
    /// object's new application.
    pub(super) fn reparent_body_less(
        &mut self,
        class: ClassReference<'gc>,
        declaration: MixinId,
        reparenting: &Reparenting<'gc>,
        superclass: Option<(ClassReference<'gc>, Rc<MixinApplication>)>,
    ) -> Rc<MixinApplication> {
        let current = self
            .heap
            .mixin(declaration)
            .expect("internal: a re-parented declaration has a row");
        let Some((&(last_index, last_applied), below)) = reparenting.applied.split_last() else {
            unreachable!("internal: a body-less declaration writes a `<:` clause");
        };
        // Every applied class was found a class object before the commit
        // began (`Mutator::clause_reparentings`).
        let Ok(top) =
            crate::interpreter::nested::build_applied_levels(self, &current, superclass, below)
        else {
            unreachable!("internal: every applied class is a class object");
        };
        let Some((named, named_application)) = last_applied.as_object().and_then(|named| {
            self.heap
                .class_application(named)
                .map(|application| (named, application))
        }) else {
            unreachable!("internal: every applied class is a class object");
        };
        let new = self.apply_mixin(
            &named_application.mixin,
            top.as_ref().map(|(_, application)| application),
        );
        let clause = crate::mixin::AppliedClause {
            declaration: current,
            index: last_index,
            named_builtin: self.heap.class_builtin(named),
        };
        let enclosing = self.heap.class_enclosing(named);
        let declaring_class = self.heap.class_declaring(named);
        let written = top
            .as_ref()
            .is_none_or(|(level, _)| self.heap.class_set_superclass(class, *level))
            && self.heap.class_install_application(class, Rc::clone(&new))
            && self.heap.class_set_applied_clause(class, clause)
            && self.heap.class_set_named_class(class, named)
            && self
                .heap
                .class_set_lexical_context(class, enclosing, declaring_class);
        if !written {
            unreachable!("internal: a body-less declaration's class object is a level");
        }
        new
    }

    /// The reflected name of the declaration `id`'s current version.
    fn declaration_name(&self, id: MixinId) -> String {
        self.heap
            .mixin(id)
            .map_or_else(String::new, |mixin| mixin.reflected_name())
    }
    /// Records, or clears, §3.1.1's enclosing-object verdict on `class`, a
    /// nested class object re-applied over a chain an install may have
    /// changed, as `nested::after_clause` records it when the class object
    /// is made: decided, not signalled, so that only an instantiation
    /// refuses.
    ///
    /// A body-less declaration's class object is held to its declaration's
    /// enclosing object and to its level's, as `nested::body_less_class`
    /// holds it; a root and a level a `<:` clause added below a class have
    /// no verdict of their own.
    pub(super) fn redecide_value_class(
        &mut self,
        class: ClassReference<'gc>,
        inherits_value: bool,
    ) {
        use crate::interpreter::value_class::{check_body_less_value_class, check_value_class};
        let refusal = if self.heap.class_body_less_declaration(class).is_some() {
            let Some(enclosing) = self
                .heap
                .class_declaration_scope(class)
                .and_then(|scope| scope.enclosing)
            else {
                return;
            };
            if inherits_value {
                check_body_less_value_class(self, enclosing, class)
            } else {
                None
            }
        } else {
            if self.heap.class_applied_origin(class).is_some() {
                return;
            }
            let Some(enclosing) = self.heap.class_enclosing(class) else {
                return;
            };
            if inherits_value {
                self.heap
                    .class_superclass(class)
                    .and_then(|superclass| check_value_class(self, enclosing, superclass))
            } else {
                None
            }
        };
        let written = match refusal {
            Some(refusal) => self.record_value_class_refusal(class, refusal),
            None => self.heap.class_clear_value_refusal(class),
        };
        if !written {
            unreachable!("internal: a re-applied class is a class object");
        }
    }
}
