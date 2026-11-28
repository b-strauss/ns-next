# Run-time semantics

The operations a program's meaning is built from, as abstract operations
in the manner of ECMA-262: each is named with its parameters and given as
numbered steps, and a step that can fail says what it signals. They state
only what a program observes -- what is evaluated in which order, what
answers and what is signalled with which text -- and never how the
implementation achieves it: caches, compiled code and the heap's layout
change none of these steps, and the part "VM invariants"
([the chapter](vm-invariants.md)) states what they must keep.

Every operation runs in an *activation*: the receiver, the *current
class* -- the class the running method was found in, or the block's
making activation's -- the variables, and the activation it answers to.
"Signal *e*" runs [HandlerSearch](#r-runtime.handler-search) for a new
`Error` with the text *e*.

## Lookup and sends

### Lookup(class, selector, access)

r[runtime.lookup]
*access* is `public`, `protected` or `any`.

1. Let *C* be *class*.
2. If *C* is `Top`, answer nothing.
3. If the mixin of *C* declares a member -- a method, a slot accessor or
   a nested class -- named *selector*:
   1. If its access is `public`, or *access* is `any`, answer it.
   2. If its access is `protected` and *access* is `protected`, answer
      it.
   3. If its access is `protected` and *access* is `public`, answer
      nothing.
   4. Its access is `private`: go on to step 4.
4. Let *C* be the superclass of *C*, and go to step 2.

The levels a `<:` clause adds are classes of the chain like any other, met
after the declaration's own and before its superclass's
([`semantics.send.ordinary`](language/semantics.md#r-semantics.send.ordinary)).

### Send(receiver, selector, arguments)

r[runtime.send]
The ordinary send, after its receiver and its arguments have been
evaluated, left to right
([`semantics.send.evaluation-order`](language/semantics.md#r-semantics.send.evaluation-order)).

1. Let *m* be [Lookup](#r-runtime.lookup)(the class of
   *receiver*, *selector*, `public`).
2. If *m* is nothing, answer
   [DoesNotUnderstand](#r-runtime.does-not-understand)(*receiver*,
   the class of *receiver*, *selector*, *arguments*).
3. Answer [Invoke](#r-runtime.invoke)(*m*,
   *receiver*, the class whose mixin declares *m*, *arguments*).

A super send is Send with step 1's class the superclass of the current
class and *access* `protected`, and *receiver* the current receiver; its
does-not-understand starts at that superclass.

### ImplicitReceiverLookup(activation, selector)

r[runtime.implicit-receiver]
A send with no receiver
([`semantics.send.implicit-receiver`](language/semantics.md#r-semantics.send.implicit-receiver)).
Which of steps 1 to 3 applies is decided from the source, before
anything runs.

1. If a variable named *selector* is in scope in the enclosing methods
   and blocks, read it and answer its value. If *selector* is one
   keyword and a mutable local named by the name before its colon is in
   scope there, write the argument into the innermost such local and
   answer the argument
   ([`semantics.send.local-setter`](language/semantics.md#r-semantics.send.local-setter));
   a parameter and an immutable local are passed by.
2. If a class declaration *N* lexically enclosing the send declares a
   member named *selector*, the innermost such *N*, let *k* be its depth
   outwards from the declaration the running code was written in; answer
   [OuterSend](#r-runtime.outer-send)(*activation*,
   *k*, *selector*, the arguments).
3. Answer [OuterSend](#r-runtime.outer-send)(*activation*,
   0, *selector*, the arguments): a self send.

### OuterSend(activation, depth, selector, arguments)

r[runtime.outer-send]
1. Let *o* be [EnclosingObject](#r-runtime.outer)(*activation*,
   *depth*), and *N* the declaration at *depth*.
2. If *N*'s own mixin declares a `private` member named *selector*,
   answer [Invoke](#r-runtime.invoke)(that member,
   *o*, *N*'s class at that level, *arguments*).
3. Let *m* be [Lookup](#r-runtime.lookup)(the class of *o*,
   *selector*, `protected`).
4. If *m* is nothing, answer
   [DoesNotUnderstand](#r-runtime.does-not-understand)(*o*,
   the class of *o*, *selector*, *arguments*).
5. Answer [Invoke](#r-runtime.invoke)(*m*, *o*, the
   class whose mixin declares *m*, *arguments*).

### Invoke(method, receiver, class, arguments)

r[runtime.invoke]
1. Make an activation of *method* with *receiver* as its receiver, *class*
   as its current class and the parameters bound to *arguments*; it
   counts toward [the depth limit](#r-runtime.depth-limit).
2. Run the method's statements in it.
3. If a `^e` runs, answer the value of *e*; if the statements end, answer
   *receiver*. A slot accessor answers the slot's value, or stores its
   argument and answers *receiver*.

### DoesNotUnderstand(receiver, start, selector, arguments)

r[runtime.does-not-understand]
1. Let *h* be [Lookup](#r-runtime.lookup)(*start*,
   `doesNotUnderstand:`, `any`). `Object` declares one, so *h* is a
   method.
2. Let *message* be a new `Message` whose `selector` is *selector* and
   whose `arguments` is an `Array` of *arguments*.
3. Answer [Invoke](#r-runtime.invoke)(*h*,
   *receiver*, the class whose mixin declares *h*, « *message* »).

`Object`'s `doesNotUnderstand:` signals a `MessageNotUnderstood` holding
*message* and *receiver*, whose text is `an object does not understand
#selector`, and answers the value it is resumed with.

## Classes and objects

### EnclosingObject(activation, depth)

r[runtime.outer]
The object an outer send at *depth* sends to
([`semantics.outer.lexical`](language/semantics.md#r-semantics.outer.lexical)).

1. Let *o* be the receiver of *activation* and *C* its current class --
   or, for the code of a declaration with no body, the declaration's own
   enclosing object and declaring class, which its class object records.
2. Repeat *depth* times: let *o* be the enclosing object of *C*, and *C*
   the declaring class of *C*.
3. Answer *o*.

### ApplyMixin(mixin, superclass, enclosing)

r[runtime.apply-mixin]
Makes a class object
([`semantics.mixin.application`](language/semantics.md#r-semantics.mixin.application)).

1. If *superclass* is not a class object, signal `the superclass clause
   of class C did not evaluate to a class object`, or for the *n*-th `<:`
   clause's class `mixin application clause n of class C did not
   evaluate to a class object`.
2. Let *K* be a new class object, the application of *mixin* to
   *superclass*, with *enclosing* as its enclosing object and the class
   that declares *mixin*'s declaration as its declaring class, and a new
   metaclass applying *mixin*'s class side to `Class`.
3. If *mixin* inherits from `Value` through *superclass* and *enclosing*
   is not a value object, record on *K* the refusal of
   [`semantics.value.conditions`](language/semantics.md#r-semantics.value.conditions)'s
   third condition; *K* is still made.
4. Answer *K*.

A nested class's accessor, sent the first time to an object *r*,
evaluates the declaration's superclass clause with *r* as `self`, runs
ApplyMixin for it and for each `<:` clause in turn with *r*'s
corresponding enclosing object, and keeps the last class for *r*; every
later send to *r* answers it.

### Instantiate(class, arguments)

r[runtime.instantiate]
The primary factory of *class*
([`semantics.instantiation.factory`](language/semantics.md#r-semantics.instantiation.factory)).

1. If *arguments* are not as many as the factory's pattern takes, signal
   `wrong number of factory arguments: factory takes n, got m`.
2. If *class* carries a refusal -- a value condition, or a module root's
   clause that a host built past the resolver -- signal it, as
   `cannot instantiate C: ` and its reason; nothing is made.
3. Let *o* be a new object of *class*, every slot of it and of its
   superclasses `nil`. If *class* inherits from `Value`, *o* is marked
   under construction.
4. Run *class*'s initializer on *o* with *arguments*, which runs, in
   this order, the superclass clause's message, the superclass's
   initializer on *o*, each `<:` clause's message and its level's
   initializer, the slot declarations in order and the initializer's
   statements
   ([`semantics.instantiation.initializer-order`](language/semantics.md#r-semantics.instantiation.initializer-order)).
   A clause's message whose selector is not its class's primary
   factory's signals `the superclass clause of class C sends #s, but the
   primary factory of class B is #t` (or `a mixin application clause of
   class C sends`...).
5. Clear *o*'s mark, if it carries one, and answer *o*. A run of the
   initializer that does not return -- an unwind past it, a signal it
   does not handle -- never reaches this step, and *o* stays marked for
   good; storing *o* while it is marked is allowed.

### SlotRead(object, slot) and SlotWrite(object, slot, value)

r[runtime.slot.read]
A getter answers the value of *slot* in *object*. A lazy slot's getter,
when the slot holds `nil`, evaluates its expression with *object* as the
receiver, stores the value and answers it
([`semantics.slot.lazy`](language/semantics.md#r-semantics.slot.lazy)).

r[runtime.slot.write]
A setter stores *value* in *slot* and answers *object*. An initializing
store into an instance of a class that inherits from `Value` signals,
for a *value* that is no value object, `cannot store x in C's slot s: a
value object is deeply immutable, so every slot it declares holds a
value`.

r[runtime.slot.guard]
A running activation keeps the slots of the class it was entered in. If
[ForwardIdentity](#r-runtime.forward-identity) has replaced its receiver with
an object that no longer has those slots, its next access to one of them
signals `x no longer has the slots of C, which the running code was
compiled for`; one whose replacement still has them -- an instance of a
subclass -- reads and writes them in the replacement.

### ValueEquality(left, right), ValueHash(value) and IsValue(object)

r[runtime.value.equality]
`Value`'s `=`, sent to *left* with *right*
([`semantics.value.identity`](language/semantics.md#r-semantics.value.identity)).

1. Walk the pairs of objects the comparison reaches, from *left* and
   *right*: each pair's classes, then its slots in declaration order,
   each unfolded before the next -- the order `valueEquals:` compares
   them in -- a pair already met assumed equal. Where a pair below the
   root has a receiver's side whose `=` is not one this operation
   computes itself -- `Value`'s; a number's, a String's or a Symbol's,
   of the kernel's own class; a class object's `Behavior>>=` -- as a
   class that declares its own `=` has, compare the slots instead in
   declaration order, each pair by sending `=` to the receiver's slot,
   which decides it, under construction or not, and runs this operation
   again for a pair that is `Value`'s. The steps below hold every other
   pair.
2. If the pair's receiver's side is under construction
   ([`semantics.value.under-construction`](language/semantics.md#r-semantics.value.under-construction)),
   signal `cannot compare an instance of C: it is still under
   construction, and an instance of a value class is a value only once
   its factory has returned`, naming that object's class, before anything
   else is compared.
3. Otherwise, where the receiver's side is a value whose class side
   declares `==` of its own, that `==` compares the two classes, as
   `valueEquals:` sends it, and the slots are compared by sending, as
   step 1 does; `valueEquals:` reads each side's slots through readers
   that signal step 2's text for one under construction. Otherwise the
   classes are compared by `Behavior>>==`: if they cannot be equal, the
   pair is unequal, whatever the state of its argument's side.
4. Otherwise, if its argument's side is under construction, signal step
   2's text naming that object's class.
5. An immediate and another instance of its class -- `nil` and
   `UndefinedObject new` -- are equal, neither having any slot.
6. The walk ends at the first pair that signals or is unequal, in its
   order, answering `false` for an unequal one, and `true` when none is.

r[runtime.value.hash]
`Value`'s `hash`, sent to *value*, is defined over the first 4096 nodes
of *value*'s unfolding, visited breadth-first: *value* first, then the
nodes on a queue in the order they were put on it.

1. *value* is unfolded: it adds its slot count, 0 for a state that is no
   slots, and puts its slots on the queue in declaration order.
2. Any other node is unfolded the same way where both its `=` and its
   `hash` are `Value`'s; otherwise it adds what its own `hash` answers --
   `nil`'s 0, a Boolean's, a number's, a String's, a Symbol's, a class
   object's, or the method a class declares -- and puts nothing on the
   queue.
3. The sum starts at 0 and, after each addition, is masked to its low 60
   bits (`bitAnd: 2^60 - 1`), so it is a `SmallInteger` from 0 to
   2^60 - 1 whatever the nodes add.
4. Once 4096 nodes have been visited, the sum so far is the hash, so that
   a value whose state reaches itself has one; two equal values unfold to
   the same sequence of nodes however their states share structure -- a
   ring of one value and an equal ring of two -- and hash alike.
5. If *value*, or a node step 2 unfolds, is under construction, signal
   `cannot hash an instance of C: it is still under construction, and an
   instance of a value class is a value only once its factory has
   returned`, naming that object's class, instead of answering. A node
   whose `hash` is its own answers as that method does.
r[runtime.value.is-value]
Whether *object* is a value object, as an `Ephemeron`'s `key:` and a
`WeakMap`'s `at:put:` ask of a key before refusing a value: an object
under construction is neither a value nor an ordinary object yet, and the
question signals `cannot tell whether an instance of C is a value: it is
still under construction, and an instance of a value class is a value
only once its factory has returned` for it, naming its class, instead of
answering.

## Unwinding and exceptions

### Unwind(target, value)

r[runtime.unwind]
Ends every activation above *target* and makes *target* answer *value*:
a non-local return's home, the `on:do:` a handler's `return:` ends, or
the activation a `retry` runs again.

1. If *target* has already returned, signal `cannotReturn: the enclosing
   method activation has already returned`, a `CannotReturn` carrying
   *value*.
2. For each activation above *target*, innermost first: if it is
   running the body of an `ensure:` or an `ifCurtailed:`, run that
   block's argument, as an ordinary block activation, before ending it.
3. End the activations above *target*, and let *target* answer *value*.

A block run in step 2 may itself unwind or signal; its own unwind
starts from where it runs.

### HandlerSearch(exception)

r[runtime.handler-search]
1. Let *a* be the activation that signalled *exception*.
2. Walk outwards from *a*: for each activation of an `on:do:` whose
   handler is enabled, if its class answers `true` to `handles:
   exception`, disable the handler, run its handler block with
   *exception* in a new activation above *a*, and when the block answers
   *v*, [Unwind](#r-runtime.unwind) to that `on:do:` with *v*.
3. If no handler is found, run *exception*'s `defaultAction`, and answer
   what it answers from the `signal`.

A handler is enabled again when its block ends or the exception is
resumed. An exception signalled while a handler block runs is searched
for from outside that handler's own `on:do:`.

## Change at run time

### ForwardIdentity(old, new)

r[runtime.forward-identity]
The bulk `become:`, reached only through the VM mirror by the install.

1. If *old* and *new* are not `Array`s of one size, or an object stands
   twice in *old*, or both in *old* and in *new*, refuse: change nothing.
2. If an element of either is anything but an ordinary instance of a
   program's class -- a number, a string, a symbol, a class object, a
   block, an `Array`, a mirror's withheld value, an instance of the core
   library's classes -- refuse: change nothing.
3. For each pair, give the new object the old one's identity hash.
4. Make every reference to each old object, everywhere, a reference to
   its new one, at once.

### Install(edits)

r[runtime.install]
The install of a list of builders' edits: `installAtomically:` given the
list, or a `ClassDeclarationBuilder`'s `install`, the list of it alone
([`semantics.reflection.atomic-install`](language/semantics.md#r-semantics.reflection.atomic-install)).
A builder `fromSource:` in the list declares a module
([`runtime.install.module`](#r-runtime.install.module)), prepared in step
2 after the edits and committed with them in step 5.

1. If two builders of the list edit one class, signal `two builders edit
   Inner` and change nothing. If a builder edits a class another builder
   of the list removes or replaces, or one nested in it, signal `the
   builder of Inner edits a class the builder of Outer removes` (or
   `replaces`) and change nothing. If a builder's class is the core
   library's, signal and change nothing; if an install removed it, or a
   class it is nested in, signal `Inner was removed by an install, and an
   obsolete class is not edited` and change nothing.
2. Prepare each module the list edits once: splice the staged edits of
   its builders into the module's current text, in the list's order,
   each starting from the text the one before it left, and compile the
   staged declarations against the result; if one does not compile,
   signal and change nothing. A module's root's header begins at its
   first metadata comment, so it holds the root's `:module:`. A new name
   -- the header's, or the one `name:` staged, which for a root rewrites
   the last part of its `:module:` id too -- or a new access of a nested
   class compiles its enclosing declaration with it, which answers the
   class under its name and access; the class keeps its identity, its
   class objects, its instances and the classes nested in it, and the
   old name is no longer understood. If the new name is no identifier,
   signal `Inner cannot be renamed to +, which is no identifier`; if the
   enclosing class already declares a member of the new name, signal
   `Outer already has a member named Sibling`; and change nothing.
   Nested classes are paired with the old ones by their names: a class
   text a builder adds under a name its class declares replaces that
   class, which keeps its identity, its class objects and its instances;
   one under a new name is a new class; a text that is not one class
   declaration signals `addFromSource: of Outer's nested classes takes
   one class declaration` and changes nothing. A class no longer declared
   is removed and lives on obsolete: its enclosing class no longer
   understands its name, and its class objects, its instances and its
   subclasses keep running its last version until nothing reaches them.
3. If a root's new text declares another id or version, the module keeps
   its path, its root and its class objects and takes that identity,
   held to the loader's rules
   ([`semantics.module.identity`](language/semantics.md#r-semantics.module.identity)):
   a malformed declaration, an id under `newspeak.`, and an id and
   version another module of the world has -- or an id another has where
   either has no version -- signal the loader's message, the last naming
   both paths, ``two modules are `com.acme.Util@2.0.0`: v1/Util.ns and
   v2/Util.ns``, and change nothing. A module whose id the project gave
   (`as`) keeps that id, a rename replacing its last part, and takes the
   version its text declares. The manifest a program holds answers the
   module under the id it was assembled with.
4. If a staged declaration's superclass clause or `<:` clauses differ
   from its current ones, evaluate them for each of its class objects
   ([`runtime.install.superclass`](#r-runtime.install.superclass)). If
   anything was installed, or a module loaded, since step 2 -- the
   install epoch moved -- or a class object was made meanwhile, signal
   and change nothing
   ([`runtime.install.epoch`](#r-runtime.install.epoch)).
5. At once and running no code, for every module's edits together: make
   each new mixin the mixin of every application of the old one; for
   every instance whose layout changed, make a new instance of the new
   layout holding each slot of the same name and `nil` in each new one,
   under construction exactly when the old instance was
   ([`semantics.value.under-construction`](language/semantics.md#r-semantics.value.under-construction))
   and its class still inherits from `Value`, and
   [ForwardIdentity](#r-runtime.forward-identity) the old instances to
   them; an instance under construction whose class no longer inherits
   from `Value` is under construction no more, whether its layout changed
   or not. Then move the install epoch.
6. A running activation finishes with the code it began with; every send
   after step 5 finds the new methods.

r[runtime.install.superclass]
An install whose staged declaration writes another superclass clause or
other `<:` clauses than its current version, or gains or loses a body of
its own, evaluates the new clauses, before it installs anything, once for
every class object of the declaration the program reaches when the
install begins, in the order the class objects were made -- a class
object nothing reaches is never evaluated, however long the memory
still holds it: each as program code in the enclosing object its
declaration was evaluated in, as part of the class that declares it,
against the world as it is before the install, and caching nothing. A
clause that signals ends the install with what it signals, changing
nothing. If an answer is no class, signal
`the superclass clause of Inner answered 3, which is no class`; if it
would make a class inherit from itself, signal `the superclass clause of
Base answers a class that inherits from Base`; and change nothing. The
commit then re-parents each class object over what its clauses answered
-- the class object keeping its identity, its `<:` levels built anew --
migrates every instance whose layout changed by slot name, a slot of a
level it lost gone and one of a level it gained `nil`, and runs no
initializer. A mixin application without a body of its own is
re-parented as any class: its class object, the level its last clause
adds, becomes the level of the new last clause. A declaration that loses
its body keeps its class objects, each becoming the level its last clause
adds, and one that gains a body keeps them too, each becoming the class
of its own body over the levels its clauses add. A `<:` level that names
a class the install re-parents applies, once installed, what that class
applies then, in that class's enclosing object and declaring class, as
the clause's evaluation would apply it (§6.2): a class that lost its body
gives the level its last clause's mixin, one that gained a body its own
mixin; every instance over the level migrates as above, and every class
over it has the value-class verdict (§3.1.1) decided again, as every
class over a re-parented class has. An install that would change the
clauses, or gain or lose the body, of a declaration a class object of
which is being created -- its superclass clause still running, the
install typically called from it -- signals `Inner is being created, and
an install does not change its superclass meanwhile` before it installs
anything, and the creation goes on over its clause's answer. A class
re-parented onto or off a builtin keeps its instances ordinary objects,
as its fresh instances are, since a subclass of a builtin takes none of
its representation. A module's root evaluates nothing: it takes the superclass its new header names,
`Object`, `Value` or `Value` applied over `Object`.

r[runtime.install.epoch]
Anything installed, or any module loaded, between an install's
preparation and its commit -- by one of its own superclass clauses as by
anything else -- fails the commit: signal `an install ran between the
preparation and the commit of Inner's install; nothing was installed`.
So does a class object of a declaration whose clauses the install
evaluates that was made while they were evaluated and that the program
still reaches at the commit, whose own clauses were never evaluated: signal `a class object of Inner was made while its
superclass clauses were evaluated; nothing was installed`. Either way
nothing changes. An install of no builders installs nothing and moves
nothing, so it fails no install it runs within.

r[runtime.install.module]
`ClassDeclarationBuilder fromSource:` given a module's text checks it as
the install would, and the builder's install declares the module:

1. If the text is not one class declaration -- none, as an empty text or
   a comment alone, two, or one followed by a member -- signal `fromSource: takes one class declaration`; if it
   does not parse, signal `the module's source does not parse: <message>
   (line <n>)`; if it does not resolve, signal `the module's source does
   not resolve: <message> (line <n>)`; and change nothing.
2. Its identity is what its `:module:` declares, held to the loader's
   rules ([`semantics.module.identity`](language/semantics.md#r-semantics.module.identity)),
   or without one its class's name and no version; it has no path. A
   malformed declaration and an id under `newspeak.` signal the loader's
   message; so do an id and version another module of the world or of the
   same install has, or an id another has where either has no version,
   a module without a path named `a module declared from source`: ``two
   modules are `com.acme.Util@1.0.0`: v1/Util.ns and a module declared
   from source``. Either way
   nothing changes.
3. The commit makes the module one of the world's, its text, every
   character of it, its current text, and the builder stands for its
   declaration from then on; it moves the install epoch, as a module's load
   does. The manifest a program holds does not answer it.

Before its install the builder stages nothing: reading either of its
sides, or staging a new header or name through its header, signals `a
ClassDeclarationBuilder from source stages nothing before its install`;
its header's `source` answers `nil`, and so does its declaration.

## Capabilities and limits

r[runtime.vm-mirror]
The VM mirror -- the object every primitive is a message to (Bracha
§8.3.1) -- is reached only by the core library's own modules. No program
can name it, its class or `Top`; no mirror, message, exception or
forward hands it out; and no image can carry it.

r[runtime.depth-limit]
A run holds at most 100,000 activations, counted as
[`semantics.depth-limit`](language/semantics.md#r-semantics.depth-limit)
says. The activation that would exceed it is not made: `stack overflow:
recursion too deep` is signalled instead, and the handlers and `ensure:`
blocks of what follows run within 1,000 activations more; past those, the
run ends as an unhandled error does.

## Memory

r[runtime.memory.identity-hash]
Every object answers an `identityHash`, the same for its whole life --
across every collection and every move, and from a replaced object to
its replacement under ForwardIdentity. A value object's hash follows its
`=`.

r[runtime.memory.weak]
A `WeakArray` holds its elements weakly: an element nothing else reaches
becomes `nil` at a collection, and until then it is answered as stored.

r[runtime.memory.ephemeron]
An `Ephemeron` holds its key weakly and its value and finalizer only
while its key is reached from outside ephemerons: once nothing else
reaches the key, a collection sets all three to `nil`. A chain of
ephemerons whose keys only each other reach dies together. An instance of a
program's subclass of `Ephemeron` is an ordinary object, which holds all
three strongly. The
finalizer is kept and never run.
