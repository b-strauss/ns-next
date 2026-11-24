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
`String` is an implicit-receiver send
([`semantics.send.implicit-receiver`](#r-semantics.send.implicit-receiver)),
so a class that declares or inherits a member of that name reaches its
own, and an enclosing class's member of that name is found before
`Object`'s. `Top`, the mirror classes and every other class of the core
library have no such method: a bare `Top` is not understood. Further
classes are handed out by the platform.

## Messages

r[semantics.send.evaluation-order]
A send evaluates its receiver expression first, then its arguments from
left to right, and only then looks up and invokes the method (Bracha
§5.4.1, §5.5); the lookup sees whatever the arguments' evaluation
changed. An operand keeps the value it was evaluated to even when a later
operand changes the variable it was read from. A setter send `x: e` used
as an expression answers the value of `e`, not what the setter answers.

r[semantics.send.ordinary]
An ordinary send, `receiver message`, sends the message to the object the
receiver answers. Its method is found by *lookupPublic* from the
receiver's class up its superclasses: at the first class whose mixin
declares the selector, a `public` declaration is the answer, a
`protected` one ends the search with none, and a `private` one is passed
over (Bracha §5.5, §6.3.1). A method, a slot's accessor and a nested
class are members alike. A member written with no modifier is
`protected`, so an ordinary send reaches only `public` members, and a
`protected` member hides a `public` one of a superclass from it.
Conditionals and loops -- `ifTrue:`, `and:`, `whileTrue:`,
`timesRepeat:` -- are ordinary sends to their receiver, whatever the
implementation does to them: `3 ifTrue: ['a'] ifFalse: ['b']` is not
understood, and a loop whose condition answers no Boolean signals `a
loop's condition answered 4, which is no Boolean`.

r[semantics.send.implicit-receiver]
A send written with no receiver, `m`, is looked up first along the
lexical scope: a parameter, temporary or local of the enclosing methods
and blocks named `m` is read or written; otherwise, when a class
declaration that lexically encloses the send declares a member named by
the selector, the innermost such declaration `N` decides, and the send is
the outer send `outer N m`; otherwise the send is `self m` (Bracha §5.7).
A declaration's members are the ones its own body declares: a member a
lexically enclosing class only inherits is not in scope. The lexical
scope is thus searched outwards before the receiver's superclasses are.

r[semantics.send.self]
A self send, `self m` or `(self) m`, is the outer send `outer C m` for
the class `C` whose declaration immediately encloses it (Bracha §5.8), so
it reaches the `public` and `protected` members of the receiver's class
chain and the `private` members `C` declares. `self` alone answers the
current receiver.

r[semantics.send.outer]
An outer send, `outer N m`, sends `m` to the enclosing object of the
lexically enclosing declaration `N`
([`semantics.outer.lexical`](#r-semantics.outer.lexical)). When `N`
declares a `private` member named by the selector, that member is
invoked; otherwise the method is found by *lookupProtected* from that
object's class: the first `public` or `protected` declaration is the
answer, and a `private` one is passed over (Bracha §5.9). A `private`
member is reached only through its own declaration's scope -- never by a
subclass, a superclass or another class of the same name.

r[semantics.send.super]
A super send, `super m`, sends `m` to the current receiver, looking it up
by *lookupProtected* from the superclass of the class the running method
was found in -- not of the receiver's class -- so that a chain of super
sends climbs one level at a time; a `private` method there is passed over
(Bracha §5.10).

r[semantics.send.cascade]
A cascade, `e m0; m1; ...; mn`, evaluates the receiver `e` of `m0` once,
sends each of its messages to that one object in turn and answers the
value of the last (Bracha §5.4.4). A cascade on `self` is an ordinary
send: `self foo; bar` reaches only public members.

r[semantics.send.does-not-understand]
When a send's lookup finds no method, the receiver's `doesNotUnderstand:`
-- the first defined for its class, whatever its access modifier, or for
a super send the first defined for the superclass the lookup started at
-- is invoked with a `Message` holding the selector and the arguments,
and its answer is the send's value (Bracha §5.5). `Object`'s signals a
`MessageNotUnderstood`, `an object does not understand #fooBar`, and
answers the value the exception is resumed with. `doesNotUnderstand:` is
`protected` on `Object`, so an ordinary send of it is itself not
understood. A class object's sends reach its class side's
`doesNotUnderstand:` first.

## Literals

r[semantics.literal.integer]
A number with neither a fraction nor an exponent denotes an `Integer` of
its exact value, in base 10 or in the radix written before `r`, `16r1F`
being 31 (Bracha §5.1.1). An integer is not limited in size: integer
arithmetic never overflows, and answers an `Integer` however large its
value is.

r[semantics.literal.float]
A number with a fraction or an exponent denotes a `Float`: the IEEE 754
binary64 number nearest its decimal value, `1e3` being `1000.0`. Bracha
§5.1.1 makes it an exact decimal; ns-next makes it a binary float (see
the appendix), and its exact decimal is the gap
[`semantics.gap.decimal`](#r-semantics.gap.decimal). Float arithmetic is
IEEE 754's, NaN, the infinities and signed zeros included.

r[semantics.number.exact]
Arithmetic on integers and fractions is exact: dividing two integers that
do not divide answers a `Fraction` in lowest terms. Numbers of different
classes compare by their exact values, `1 = 1.0`, and two numbers that
are equal hash alike.

r[semantics.gap.decimal]
An exact decimal number has no class and no literal in ns-next yet. A
`Decimal` class, reached by a protected accessor on `Object`, and its
literal -- a decimal number followed by `s` and an optional scale,
`2.50s`, `2.5s2`, `3s2`, `-0.1s`, never with a radix -- are decided and
not built: `2.5s2` is today the number `2.5` followed by the unary
message `s2`.

r[semantics.literal.string]
A string literal denotes a `String` holding the code points its source
text holds between the quotes, in that order, a doubled quote standing
for one (Bracha §5.1.5; see the appendix for normalization). A string is
a value object: two strings of the same code points are equal and
identical, and a string cannot be changed.

r[semantics.literal.symbol]
A symbol literal, `#name`, `#at:put:`, `#+` or `#'any text'`, denotes the
`Symbol` of that text. A symbol is a string, and two equal symbols are
one object (Bracha §5.1.6).

r[semantics.literal.boolean-and-nil]
`true` and `false` denote the unique instances of `True` and `False`, and
`nil` the unique instance of `UndefinedObject` (Bracha §5.1.2, §5.1.3).
All three are value objects.

r[semantics.gap.immutable-array]
Bracha §5.1.7 makes a tuple literal, `{e1. e2}`, a shallowly immutable
tuple, and ns-next decides it is an `ImmutableArray`, a class it does not
build yet. ns-next evaluates the elements from left to right and answers
a new `Array` of their values at each evaluation, which a program can
store into.

r[semantics.literal.block]
A block literal, `[:x | ...]`, denotes a new `Closure` at each
evaluation, which keeps the receiver, the class and the variables of the
activation it was made in (Bracha §5.1.8). Sending it `value`, `value:`,
... with as many arguments as it has parameters runs its body in a new
activation and answers the value of its last statement, or `nil` for an
empty body; any other count signals `wrong number of block arguments:
block takes 1, got 0`. Its temporaries are fresh at each invocation, and
it reads and writes the variables it closes over, not copies of them.

r[semantics.gap.pattern]
Bracha §5.1.9 makes a pattern literal a send to `Pattern`: `<_>` is
`Pattern wildcard`, `<l>` `Pattern literal: l`, a keyword pattern
`Pattern keywords: kws patterns: pats`, `Pattern` an implicit-receiver
send. ns-next builds no pattern matching: a pattern literal that is
evaluated signals an error, `a pattern literal needs pattern matching,
which ns-next does not build yet (§5.1.9)`. It fails when it is
evaluated, never when its module is compiled; the grammar is unchanged.

## Actors and asynchronous sends

r[semantics.actor]
An actor is a unit of concurrency with a heap of its own; actors share no
mutable state and communicate only by asynchronous messages, each actor
processing the messages in its mailbox one at a time, each to its end, a
*turn* (Bracha §3.7). An actor is made from a top-level mixin.

r[semantics.actor.promise]
An asynchronous send, `receiver <-: message`, evaluates its receiver,
then its message's arguments, and answers at once a promise for the
send's result (Bracha §5.6). The message is delivered as an ordinary send
in a later turn of the receiver's actor, even when that is the current
actor; when that send answers, the promise is fulfilled with its result,
and when it raises an exception, the promise is broken with it. A send to
a promise is delivered once the promise is resolved, to what it resolved
to.

r[semantics.actor.turn]
A turn is never interrupted by another message: an actor neither blocks
waiting for a message nor processes a second message before the first
returns (Bracha §3.7). How a turn waits for a promise it needs is not
decided.

r[semantics.actor.e-order]
Messages are delivered in E-Order: two messages one actor sends to
another arrive in the order they were sent, and a message sent to a far
reference before the reference is passed to a third actor arrives before
any message that actor sends to it (Bracha §5.6).

r[semantics.actor.remote-representation]
An argument and a result cross from one actor to another as their remote
representation: a value object as itself, copied or shared, since it is
deeply immutable, and any other object as a far reference to it (Bracha
§3.1.1, §3.7). An actor never holds a far reference into its own heap.

r[semantics.gap.actors]
ns-next builds none of [`semantics.actor`](#r-semantics.actor) to
[`semantics.actor.remote-representation`](#r-semantics.actor.remote-representation):
there are no actors, promises or far references. An asynchronous send
evaluates its receiver and its arguments, left to right, as an ordinary
send does, and then signals an error, ``asynchronous send of
`printString` needs actors, which ns-next does not build yet (§5.6)``,
naming its message's selector. It fails when it runs, never when its
module is compiled.

## Methods and blocks

r[semantics.method.return]
A method runs in a new activation whose receiver is the object the
message was sent to, its parameters bound to the message's arguments.
`^e` ends the activation and answers the value of `e`; a method whose
statements end without a `^` answers its receiver (Bracha §3.5, §7.2).
Its parameters are immutable, and a parameter and a local of the same
name in one method or block are a resolve error, ``method `shadow:`
declares `value` twice, as a parameter and as a local (§3.6)``. A
local, `| x |`, `| x ::= e. |` or `| x = e. |`, is initialized in the
order it is declared, `nil` when it has no initializer, and one declared
with `=` cannot be assigned: a send of its setter is not understood.

r[semantics.block.closure]
A block's activation runs with the receiver and the class of the
activation the block was made in, and reads and writes that
activation's parameters and locals for as long as the block lives, even
after the activation has returned (Bracha §3.6, §5.1.8). A block's own
parameters are immutable. Each evaluation of a block literal makes a new
closure, and each closure that outlives its making shares the variables
it closes over with every other closure made in the same activation.

r[semantics.block.non-local-return]
`^e` in a block ends the method activation the block was written in --
its home -- and answers the value of `e` from it, ending every activation
between them on the way (Bracha §5.1.8, §7.2); the blocks of
`ensure:` and `ifCurtailed:` between them run as they are passed.

r[semantics.block.cannot-return]
A `^` in a block whose home has already returned signals `cannotReturn:
the enclosing method activation has already returned`.

r[semantics.gap.tail-calls]
Bracha §3.6 eliminates tail calls to the extent that a program can
observe them, and marks it *Unimplemented*. ns-next keeps every
activation until it returns: a recursion of any kind is bounded by the
depth limit.

## Slots

r[semantics.slot.accessors]
A slot declared by a class is reached only by messages: its getter, the
slot's name, and, for a mutable slot, its setter, the name with `:`
appended, which stores its argument and answers the receiver (Bracha
§6.3.2). Both are members with the slot's access modifier and may be
overridden like any method; a slot's initialization is never a send of
its setter.

r[semantics.slot.immutable]
A slot declared `x = e.` is immutable: it is set to the value of `e` when
the instance is initialized and has no setter, so `x: 3` is not
understood.

r[semantics.slot.mutable]
A slot declared `x ::= e.` is mutable and set to the value of `e` when
the instance is initialized; a slot declared with no initializer, `x` or
`x.`, is mutable and set to `nil`.

r[semantics.slot.lazy]
A lazy slot, `lazy x = e.` or `lazy x ::= e.`, in any order with its
access modifier, holds `nil` until its getter is sent; while it holds
`nil`, the getter evaluates `e` and stores the value, without its
setter, and answers it, so an `e` that answers `nil` is evaluated again at
the next read, and one that signals leaves the slot empty (Bracha
§6.3.3). A lazy slot's expression is evaluated with the instance as its
receiver and does not see the factory's parameters: a name only a
factory parameter declares is sent to the instance.

r[semantics.slot.simultaneous]
The slots of a simultaneous clause, `|| a = e1. b = e2. ||`, and the
temporaries of a code body written so, each start as a future of their
expression; each future is computed the first time its value is needed
and all are resolved once the clause's last slot is declared, so that a
well-founded mutual recursion among them resolves (Bracha §6.3.2). A
future is its value to every observer: equal, identical, an argument
and a receiver like it. A future whose computation needs its own value
signals `divergent evaluation: a future was forced while it was being
computed`.

## Access control

r[semantics.access.public]
A `public` member is reached by every send whose lookup meets it:
ordinary, self, implicit-receiver, outer and super sends (Bracha
§6.3.1).

r[semantics.access.protected]
A `protected` member, the default for a member written without a
modifier, is reached by self, implicit-receiver, outer and super sends,
never by an ordinary send, for which it ends the lookup with no method
([`semantics.send.ordinary`](#r-semantics.send.ordinary)). The access of
a top-level class is always public.

r[semantics.access.private]
A `private` member is reached only by a send whose lexical scope is its
own declaration's: an implicit-receiver, self or outer send that names
that declaration. Every lookup that starts at a class passes it over, so
it is never overridden, never reached from a subclass or a superclass,
and a private nested class is unreachable from outside.

## Instantiation

r[semantics.instantiation.factory]
A class is instantiated by sending it its primary factory message, `new`
when its declaration names none (Bracha §6). The factory makes a new
object, distinct from every other, with one slot for every slot its
class and superclasses declare, each `nil`, runs the instance
initializer on it with the factory's arguments, and answers it. A
factory sent with a number of arguments its pattern does not take
signals `wrong number of factory arguments: factory takes 1, got 2`.
The factory's parameters are immutable and are in scope in the
initializer -- the slot declarations, the initializer's statements and
the superclass and mixin clauses' messages -- and nowhere else in the
class.

r[semantics.instantiation.initializer-order]
The instance initializer runs, in this order: the superclass clause's
message is evaluated with the factory's arguments; the superclass's
initializer runs with that message, wholly; the class's slot
declarations are executed in the order they are written; then the
initializer's statements run; and the factory answers the instance
(Bracha §6.3). An applied mixin's level runs between the superclass's
and the class's own
([`semantics.mixin.clause-order`](#r-semantics.mixin.clause-order)).

r[semantics.class-side]
A class's class side, `: ( ... )` after its body, declares the methods of
its class object, the instance of its metaclass; the primary factory is
one of its members, and a class-side method sends it as `new` or with
its pattern (Bracha §6.3). A metaclass is a direct subclass of `Class`:
a subclass does not inherit its superclass's class-side methods, and a
metaclass does not understand the factory of its class. `self class`
on the class side is the metaclass, and the class of a class is its
metaclass. A class-side method's lexical scope is its class's: an
implicit-receiver or outer send reaches the enclosing declarations as
an instance method's does.
