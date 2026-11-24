# Semantics

What a module means when it runs: the objects it makes, the messages they
exchange and what each construct of the [Grammar](grammar.md) evaluates
to. Bracha §3 to §8 is the source; a rule restates his where ns-next
follows him, and the operations its rules rely on -- a lookup, a send, an
instantiation -- are written out step by step in the part "Run-time
semantics".

An error a rule says is *signalled* is an `Error` a handler can catch;
one left unhandled ends the program with exit status 1 and a line
`Uncaught Newspeak error: ` followed by its text. An error a rule says is
*a resolve error* is found before anything of the program runs, and
refuses the program with a line `resolve error: ` followed by its text.

## Objects and values

r[semantics.object]
Every value a program computes is an object: numbers, booleans, `nil`,
strings, blocks, classes and modules alike. An object computes only in
response to a message sent to it, and every object is an instance of a
class, which determines its slots, its methods and its nested classes
(Bracha §3.1). Two objects are identical, `==`, when they are the same
object; every object but a value object has an identity of its own,
distinct from every other object's.

r[semantics.value]
A *value object* is deeply immutable: nothing reachable from it can
change. An object is a value object when it is a number, a boolean,
`nil`, a string, a symbol or a module definition, or when its class
inherits from `Value` and the conditions of
[`semantics.value.conditions`](#r-semantics.value.conditions) hold for it
(Bracha §3.1.1).

r[semantics.value.conditions]
An instance of a class that inherits from `Value` is made only when it
can be a value object; otherwise the instantiation signals an error named
under the class's primary factory, and nothing is made. Four conditions
are checked, each where the instance would come into being:

1. No class from the instance's class up to its root declares a mutable
   slot: ``cannot instantiate Counter: a value object is deeply
   immutable, and it declares the mutable slot count`` (`X declares` in
   place of `it declares` when the slot is an inherited class's `X`'s).
2. No class of the chain defines `==` but `Value`: `cannot instantiate P:
   a value class may not define ==, and Q defines it`.
3. Every enclosing object of the class is a value object: `cannot
   instantiate P: a value class's enclosing objects must be values, and
   its enclosing object, a Counter, is not one` (naming the level whose
   enclosing object fails when it is not the class's own).
4. Every slot the instance's classes declare is initialized with a value
   object: storing anything else signals `cannot store an Array of 2 in
   Holder's slot elements: a value object is deeply immutable, so every
   slot it declares holds a value`.

A class that breaks a condition but is never instantiated never fails.

r[semantics.value.identity]
A value object has no identity of its own: `Value` defines `==` as `=`,
and `=` compares two values of the same class slot by slot, so two value
objects with equal state are equal, identical and hash alike, and a value
is never equal to an instance of another class. Numbers compare by
value across their classes, `1 = 1.0`, and so are identical too
(Bracha §3.1.1: if `Value` declares an identity method, it answers what
`=` answers).

## Classes and enclosing objects

r[semantics.class.per-enclosing-object]
A nested class belongs to an instance of its enclosing class, not to the
enclosing class. A class declaration nested directly in a class `OC`
defines a method of `OC` with the class's name and access modifier; the
first time it is sent to an instance `r`, it evaluates the declaration
with `r` as the enclosing object and answers the class it made, and every
later send to `r` answers the same class object (Bracha §6). Two
instances of `OC` therefore have two distinct nested classes, and each
instance made by one of them reaches its own `r`. Since the nested class
is reached by a message, a subclass of `OC` may override it -- with a
method, a slot or another nested class.

r[semantics.class.declaring]
Every class object records its *declaring class*, the class whose
declaration lexically holds its own, and its enclosing object, an
instance of that declaring class or of a subclass of it. A top-level
class -- a module definition -- has `nil` as its enclosing object
(Bracha §3.3). A class's metaclass has the class's enclosing object.

r[semantics.enclosing-object]
The enclosing object of a method's activation is its receiver; that of a
block's activation is the activation the block was made in. For an
object `o` and a class `C` of the chain of `o`'s class, the *n*-th
enclosing object of `o` with respect to `C` is `o` for *n* = 0 and, for
*n* > 0, the enclosing object of the class of the chain of the
*(n-1)*-th one that is an application of the mixin of `C`'s *(n-1)*-th
lexically enclosing declaration, the lowest such class when there are
several (Bracha §3.3). It is the object an implicit-receiver send that
finds its method in that declaration's scope is sent to.

r[semantics.superclass.clause]
A class's superclass is computed, not fixed: its superclass clause is an
expression evaluated when the class object is made, with the enclosing
object as `self`, as if it stood in an instance method of the enclosing
class (Bracha §6.1). A class written without a clause has `Object` as
its superclass. The clause's receiver -- implicit, `self`, `super` or
`outer N` with a unary message -- must answer a class object, or the
class's creation signals `the superclass clause of class Derived did
not evaluate to a class object`; its message is sent only when the class is
instantiated, and must be the superclass's primary factory's selector,
or the instantiation signals `the superclass clause of class Derived
sends #new, but the primary factory of class Base is #withBase:`.

r[semantics.mixin.application]
Every class is the application of its mixin -- what its declaration's
body defines -- to its superclass. A mixin application clause, `S <: M`,
applies the mixin of the class `M` answers to the class `S` answers, and
makes a class with `M`'s mixin and `S` as its superclass, whose
enclosing object is `M`'s (Bracha §6.2). Each `<:` clause of a
declaration adds one such level, left to right, and the declaration's
own body, when it has one, is applied over the last. A clause that does
not answer a class object signals `mixin application clause 1 of class
Applied did not evaluate to a class object`. The same mixin may be applied
more than once in one chain, each time a level of its own; the mixins a
class `M` was itself built by applying are not applied again: only `M`'s
own mixin is.

r[semantics.mixin.clause-order]
An instance of a class whose declaration applies mixins is initialized
level by level, from the superclass up: the superclass clause's message
is evaluated with the factory's arguments and the superclass's
initializer runs; then, for each `<:` clause in turn, its message is
evaluated with the same factory arguments just before the initializer of
the level it adds runs; then the declaration's own initializer runs
(Bracha §6.2, §6.3). A clause message's selector must be the applied
class's primary factory's, or the instantiation signals `a mixin
application clause of class Applied sends #new, but the primary factory
of class Marker is #withTag:`. A declaration with no body,
`class C = S <: M.`, is the class its last clause adds, and its primary
factory takes the selector of that clause's message.

r[semantics.mixin.standalone]
A level a `<:` clause added that a program instantiates from outside the
declaration -- reached through a mirror, as a class of its own -- is an
ordinary application of its mixin: its primary factory is its mixin's,
and an instance runs its mixin's initializer, never the clause's
message.

## `outer`

r[semantics.outer.lexical]
`outer N` names the enclosing object, with respect to the class whose
code is running, of the lexically enclosing declaration `N`: the walk
starts at the class the running code was declared in and follows each
class object's declaring class and enclosing object outwards until it
reaches `N`'s level, never the receiver's class or its superclasses
(Bracha §5.9). Code a `<:` clause applies starts its walk at the level
that clause added, whose enclosing object and declaring class are those
of the class the clause names. `outer N` naming no lexically enclosing
class is a resolve error, `` `outer N` does not name a lexically
enclosing class ``.

r[semantics.outer.alone]
`outer N` with no message answers that enclosing object, the object
`outer N m` would send `m` to, without sending anything
([`syntax.outer.alone`](grammar.md#r-syntax.outer.alone); see the
appendix).

## Modules and their roots

r[semantics.module.root-rule]
A module definition is a top-level class, with no enclosing object, so
its superclass clause has nothing to evaluate a receiver in. Its clauses
after the header's `=` are therefore one of seven: none, `Object`,
`Object new`, `Value`, `Value new`, `Object <: Value` and `Object new <:
Value`; a module written `Object <: Value` or `Value` is a value object
of its own. Any other root clause is a resolve error, for every module,
before anything runs: `a module's root class may inherit only Object or
Value, or write Object <: Value, and its superclass clause names Top`
(or `its superclass clause sends s to X, not new`, `its mixin
application clause n names X`, `its mixin application clause n sends s
to X`, `it applies Value to Value`). Bracha §6.1 evaluates the
clause as a top-level expression, failing only when it runs; see the
appendix.

r[semantics.builtin-names]
The classes of the language are reached through protected methods of
`Object`, one per class a program may name (Bracha §3.2): `Object`,
`Value`, `Boolean`, `True`, `False`, `Number`, `Integer`, `Float`,
`Fraction`, `String`, `Symbol`, `UndefinedObject`, `Array`,
`ByteArray`, `Closure`, `Exception` and `Error`. A bare name such as
`String` is an implicit-receiver send, so a class that declares or inherits a member of that name reaches its
own, and an enclosing class's member of that name is found before
`Object`'s. `Top`, the mirror classes and every other class of the core
library have no such method: a bare `Top` is not understood. Further
classes are handed out by the platform.

## Gaps

r[semantics.gap.actors]
Bracha §5.6 makes an asynchronous send, `receiver <-: message`, answer a
promise at once and deliver the message in a later turn of the receiver's
actor. ns-next builds no actors: an asynchronous send evaluates its
receiver and its arguments, left to right, as an ordinary send does, and
then signals an error, ``asynchronous send of `printString` needs actors,
which ns-next does not build yet (§5.6)``, naming its message's selector.
It fails when it runs, never when its module is compiled.

r[semantics.gap.pattern]
Bracha §5.1.9 makes a pattern literal a send to `Pattern`: `<_>` is
`Pattern wildcard`, `<l>` `Pattern literal: l`. ns-next builds no pattern
matching: a pattern literal that is evaluated signals an error, `a pattern
literal needs pattern matching, which ns-next does not build yet (§5.1.9)`.
It fails when it is evaluated, never when its module is compiled.
