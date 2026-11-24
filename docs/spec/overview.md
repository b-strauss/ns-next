# Overview of Newspeak

This chapter introduces the language informally, for a reader who knows
another object-oriented language. It states no rule; the parts after it
do, and each section here links to the rules it summarizes.

## What Newspeak is

Newspeak is a programming language in the Smalltalk tradition. It is:

- **Message-based.** Every computation is a message sent to an object,
  even an object's access to its own slots. Numbers, booleans, blocks,
  classes and modules are all objects.
- **Free of global state.** There is no global namespace and no static
  state. Whatever a piece of code can reach, it was handed: through a
  factory's arguments, an enclosing object or a message's argument. This
  makes the language a sound basis for object-capability security.
- **Built from nested classes and mixins.** Classes nest to any depth,
  every instance of an enclosing class has its own nested classes, and
  every class is a mixin that can be applied to another superclass.
- **Modular without a module system.** A module definition is a
  top-level class; a module is an instance of it. Its dependencies are
  its factory's parameters, so every dependency is explicit and a module
  can be instantiated any number of times, differently wired.
- **Reflective through mirrors.** A program reads and changes itself
  through mirrors, which are capabilities: only code handed a mirror can
  use it.

## A first program

```newspeak
class Counters = (
  |
    private total ::= 0.
  |
) (
  class Counter named: name = (
    |
      public name = name.
      public count ::= 0.
    |
  ) (
    public increment = (
      count: count + 1.
      total: total + 1.
    )
  )

  public main: platform args: arguments = (
    |
      apples = Counter named: 'apples'.
      pears = Counter named: 'pears'.
    |
    3 timesRepeat: [apples increment].
    pears increment.
    platform transcript
        show: apples name , ' ' , apples count printString;
        newLine;
        show: 'together ' , total printString;
        newLine.
  )
)
```

It prints `apples 3` and `together 4`. ns-next instantiates the module's
class with `new` and sends the instance `main:args:` with the platform,
the one object through which a program reaches the outside world, and
its command-line arguments. The sections below explain what the program
uses.

## Messages

A message is unary (`apples name`), binary (`count + 1`) or keyword
(`Counter named: 'apples'`, `anArray at: 1 put: 0`). Unary messages bind
tighter than binary ones, binary ones tighter than keyword ones, and all
binary messages share one precedence and associate to the left, so
`5 + 4 * 2` is 18 ([`syntax.precedence`](language/grammar.md#r-syntax.precedence)).
A cascade, `;`, sends several messages to the receiver of the last one
([`syntax.cascade`](language/grammar.md#r-syntax.cascade)), as the
transcript above receives four.

A message with no written receiver -- `total: total + 1` above -- is an
implicit-receiver send. It is looked up first along the lexical scope,
from the innermost class outwards, and only then through the
superclasses of `self`. `Counter` sets the `total` slot of its enclosing
`Counters` instance that way. `self`, `super` and `outer` name a receiver
explicitly ([`syntax.self`](language/grammar.md#r-syntax.self),
[`syntax.super`](language/grammar.md#r-syntax.super),
[`syntax.outer.send`](language/grammar.md#r-syntax.outer.send)).

## Classes, slots and factories

A class declaration has a header and a body
([`syntax.class-declaration`](language/grammar.md#r-syntax.class-declaration)).
The header names the primary factory and its parameters -- `named: name`
above, `new` when none is written -- and holds the instance initializer:
the slots and any code that runs when an instance is made. The body holds
methods and nested classes, and an optional class side, `:` and its
methods in parentheses, holds the class's own methods ([`syntax.class-side`](language/grammar.md#r-syntax.class-side)).

A slot declared with `=` is immutable and has only a getter; one declared
with `::=`, or without a value, is mutable and has a setter too, `count:`
([`syntax.slot-definition`](language/grammar.md#r-syntax.slot-definition)).
A slot is reached only through these messages, never directly. A lazy
slot is initialized at its first read
([`syntax.lazy-slot`](language/grammar.md#r-syntax.lazy-slot)). Members
are `public`, `protected` -- the default -- or `private`
([`syntax.access-modifier`](language/grammar.md#r-syntax.access-modifier)).
An object whose class inherits from `Value` is a value object: deeply
immutable, every slot of it immutable and holding a value object itself.

## Nested classes and enclosing objects

A nested class belongs to an instance of its enclosing class, not to the
enclosing class: two `Counters` instances have two distinct `Counter`
classes, and each `Counter` reaches the slots and methods of its own
enclosing `Counters`. A nested class is reached by a message too, so a
subclass can override it, even with a slot.

## Mixins

The superclass of a class is not fixed when the class is compiled: its
inheritance clause is an expression evaluated for each enclosing
instance ([`syntax.inheritance-clause`](language/grammar.md#r-syntax.inheritance-clause)).
Every class is therefore a mixin, and `class C = Base new <: Mixin new ( … ) ( … )`
applies the mixin of `Mixin` to the superclass `Base`
([`syntax.mixin-application`](language/grammar.md#r-syntax.mixin-application)).

## Blocks and control

A block, `[:each | each + 1]`, is a closure
([`syntax.block`](language/grammar.md#r-syntax.block)). Conditionals and
loops are messages that take blocks -- `ifTrue:`, `whileTrue:`,
`timesRepeat:` -- and `^` returns from the method the block was written
in, even from inside the block. Exceptions are signalled and handled by
messages to blocks and exception classes as well.

## Modules and programs

A source file holds one top-level class, its module definition
([`syntax.compilation-unit`](language/grammar.md#r-syntax.compilation-unit)).
A program of one module is instantiated with `new`. A program of several
modules is a project: its main module's factory receives a manifest and
asks it for each other module by its id. A module declares its id and
version in a metadata comment above its class,
`(* :module: com.acme.Json@1.0.0 *)`
([`syntax.metadata`](language/grammar.md#r-syntax.metadata)). A running
program can be saved as an image and loaded again; the part "Image
format" specifies it.

## What ns-next does not build yet

Some of Newspeak is parsed but not yet carried out, and some is not
parsed at all:

- An asynchronous send, `receiver <-: message`
  ([`syntax.eventual-send`](language/grammar.md#r-syntax.eventual-send)),
  signals an error when it runs rather than answering a promise; there
  are no actors yet.
- A pattern literal
  ([`syntax.pattern`](language/grammar.md#r-syntax.pattern)) signals an
  error when it is evaluated; there is no pattern matching yet.
- Type annotations are accepted and have no meaning
  ([`syntax.type-annotation`](language/grammar.md#r-syntax.type-annotation));
  no type system checks them.
- Object literals, chains and class declarations as expressions are not
  parsed; the appendix "Differences from Bracha's Newspeak" lists them as
  gaps.

The first two are gaps of the language's semantics
([`semantics.gap.actors`](language/semantics.md#r-semantics.gap.actors),
[`semantics.gap.pattern`](language/semantics.md#r-semantics.gap.pattern)).

Where ns-next deliberately differs from Bracha's Newspeak -- `///` line
comments, no character literals, `outer N` standing alone -- the same
appendix says how and why.
