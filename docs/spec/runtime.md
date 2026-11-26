# Run-time semantics

The operations a program's meaning is built from, as abstract operations
in the manner of ECMA-262: each is named with its parameters and given as
numbered steps, and a step that can fail says what it signals. They state
only what a program observes -- what is evaluated in which order, what
answers and what is signalled with which text -- and never how the
implementation achieves it: caches, compiled code and the heap's layout
change none of these steps, and the part "VM invariants" states what they
must keep.

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
   superclasses `nil`.
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
5. Answer *o*.

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
A `ClassDeclarationBuilder`'s `install`
([`semantics.reflection.atomic-install`](language/semantics.md#r-semantics.reflection.atomic-install)).

1. If the builder's class is the core library's, signal and change
   nothing. If its header changes the class's name, access or superclass,
   signal `the header renames Inner to Renamed; an edit keeps a class's
   name`, `the header changes the access of Inner; an edit keeps it` or
   `the header changes the superclass of Inner; an edit keeps it`, and
   change nothing.
2. Compile the staged declaration against the module's current text; if
   it does not compile, signal and change nothing.
3. At once and running no code: make the new mixin the mixin of every
   application of the old one; for every instance whose layout changed,
   make a new instance of the new layout holding each slot of the same
   name and `nil` in each new one, and
   [ForwardIdentity](#r-runtime.forward-identity) the old instances to them.
4. A running activation finishes with the code it began with; every send
   after step 3 finds the new methods.

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
