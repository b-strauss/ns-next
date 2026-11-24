# Bytecode

The code ns-next runs: a register machine whose instructions name the
registers of the frame that executes them. Every method, block,
initializer and superclass clause compiles to a *body*, and a body is a
sequence of instructions numbered from 0, its *instruction indices*. This
chapter gives the bodies and their registers, each instruction on a page
of its own, the effects every instruction has on the registers, the two
encodings of an instruction -- the wire encoding an image carries and the
eight-byte form the run loop executes -- and the rules the image decoder
checks before it accepts a body. Both encodings are normative: a change to
either changes this chapter.

What an instruction does is stated in terms of the run-time semantics'
abstract operations ([Run-time semantics](runtime.md)); this chapter adds
what the instruction itself reads, writes and decides.

## Bodies

r[bytecode.body]
A body consists of:

- its *kind*;
- its *argument count*, the number of parameters it takes;
- its *register count*, one past the highest register number of its
  frames;
- its *copied count*, the number of values a block's closure hands its
  activation, 0 for every other kind;
- its *local count*, the first scratch register;
- its source span, the half-open byte range of the source it was compiled
  from;
- its *statement map*;
- its instructions.

Three properties of a body are derived from its instructions and never
stored with it: its *outer reach*, how many frames out the deepest
by-reference access of the body or of a block it creates reaches; whether
it *addresses its own slots*; and its *family size*, one past the highest
call-site number of the body and the blocks nested in it.

r[bytecode.body.kind]
A body's kind is one of four:

- `Method`: a method's body, run in a frame whose receiver is the object
  the message was sent to, its arguments the message's;
- `Block`: a block's body, run in a frame whose receiver and current class
  are those of the activation that made the closure, its arguments the
  block's parameters;
- `Initializer`: one of a class's initializer bodies, run on a new
  instance with the factory's arguments;
- `SuperclassClause`: a class declaration's superclass clause, the
  expression Bracha's §6.1 calls `ec`, run in a frame of the enclosing
  class with no arguments, answering the class it denotes.

r[bytecode.body.statement-map]
The statement map gives, for each statement of the body, the half-open
range of instruction indices the statement compiled to. For a method or a
block the statements are its local variables' initializers in declaration
order, then the statements of its code body; for an initializer, each
slot's initializer, then each initializer expression, in that order. A
frame stands at a statement boundary when its instruction index is the
start of a range.

## Registers

r[bytecode.register.layout]
A frame's registers are numbered from 0 in this order:

1. the arguments, `0 .. argument count`;
2. the thirteen numbers of the frame's header,
   `argument count .. argument count + 13`, which no instruction names;
3. a block's copied values, `copied count` of them;
4. the named temporaries;
5. one register holding the body's indirection vector, when a block
   writes one of its variables;
6. the scratch registers, from the local count up to the register count.

The register count is at least the argument count plus 13, but for a body
past the register ceiling whose arguments leave no room for the header
([`bytecode.limit.registers`](#r-bytecode.limit.registers)), whose
registers are its arguments alone. The local count is at most the
register count.

r[bytecode.register.arguments]
No instruction writes an argument register of its own frame, nor, through
an outer local, of another.

r[bytecode.register.argument-window]
A send and an initializer invocation pass their arguments in a *window*
of contiguous registers, `argument base .. argument base + n` for *n*
arguments, each with its *source* register. Before it decides anything,
the instruction copies each source into its slot of the window where the
two differ. The window starts at or above the argument count plus 13 and
at or above the local count, and lies below the register count; no source
is another slot of the same window. The callee's frame lies on the window:
its arguments are the window's registers, its header where they end.
`InitializeInstance` passes its own body's arguments and names no window.

r[bytecode.register.leftovers]
A register the frame did not write since it was last overwritten by a
callee is dead. When a send executes, every register above its window is
dead: the callee's header and registers lie over them. No instruction
reads a register that is dead along any path from the body's entry to it
-- a loop's back edges and the entry included, where every scratch
register is dead -- and a reference lowering, an image and every
compiled body are held to this rule.

## Limits

r[bytecode.limit.instructions]
A body holds at most 2^20 = 1,048,576 instructions, so that every
instruction index, jump target and table index fits 20 bits.

r[bytecode.limit.arguments]
A body takes at most 255 parameters, and a send, a superclass clause's
message or an initializer invocation passes at most 255 arguments.

r[bytecode.limit.registers]
A body has at most 65,535 registers, the numbers a `u16` gives.

r[bytecode.limit.too-large]
A body that would pass one of these limits compiles to a body whose first
instruction loads the literal `BodyTooLarge` naming the limit it passed
-- the registers first, when its named variables do not fit, otherwise
the first its compilation met -- so that the program fails only when the
body runs, signalling an error that names the limit
([`bytecode.instruction.load-literal`](#r-bytecode.instruction.load-literal)).

## Call sites

r[bytecode.call-site]
Every send of a body has a *call-site number*, its index among the sends
of its *code family*: a method's, an initializer's or a superclass
clause's body and every block nested in it, numbered densely from 0 in
the order the compiler emits them. A body compiled by the reference
lowering numbers no send: each is *uncached*, and runs the full lookup
every time. The number identifies a send's inline cache and is never
part of the wire encoding, which says only whether a body's sends are
uncached.

## Register effects

r[bytecode.register-effects]
The table gives, for every instruction, what it reads, writes and decides
of the frame that executes it:

- `reads`: the operands whose registers it reads, in the order it reads
  them -- a send's sources, not its window, which its callee reads; an
  outer local's index, which names another frame's register, is none of
  them;
- `writes`: the operand whose register it writes, or `-` -- a send's
  destination, not its window;
- `target`: the operand that is an instruction index control may go to
  other than the next, or `-`;
- `falls-through`: whether control can go on to the next instruction;
- `outer-reach`: how many frames out it reads or writes registers by
  reference: `0`, its `depth`, or for `MakeClosure` its block's outer
  reach less one, `block-1`;
- `own-slots`: whether it addresses a slot of the receiver's level the
  frame was entered at by its index among that mixin's own slots.

A load of `BodyTooLarge` writes no register and never falls through: it
always signals.

```spec-table register-effects
instruction                  reads                                          writes       target   falls-through  outer-reach  own-slots
LoadLiteral                  -                                              destination  -        yes            0            no
LoadLiteral/BodyTooLarge     -                                              -            -        no             0            no
LoadSelf                     -                                              destination  -        yes            0            no
LoadEnclosingObject          -                                              destination  -        yes            0            no
Move                         source                                         destination  -        yes            0            no
StoreLocal                   source                                         slot         -        yes            0            no
LoadOuterLocal               -                                              destination  -        yes            depth        no
StoreOuterLocal              source                                         -            -        yes            depth        no
SendLexical                  argument_sources                               destination  -        yes            0            no
SendPublic                   receiver_register,argument_sources             destination  -        yes            0            no
SendInlinable                receiver_register,argument_sources             destination  -        yes            0            no
SendSuper                    argument_sources                               destination  -        yes            0            no
SendMirror                   mirror_register,subject_register,argument_sources  destination  failure  yes        0            no
MakeClosure                  copied                                         destination  -        yes            block-1      no
MakeArray                    element_registers                              destination  -        yes            0            no
MakeVector                   -                                              destination  -        yes            0            no
LoadVector                   vector                                         destination  -        yes            0            no
StoreVector                  vector,source                                  -            -        yes            0            no
InitializeSlot               source                                         -            -        yes            0            yes
ReturnSlotIfFilled           -                                              -            -        yes            0            yes
MakeFuture                   closure                                        destination  -        yes            0            no
ResolveFuture                source                                         destination  -        yes            0            no
LoadOwnSlot                  -                                              destination  -        yes            0            yes
InitializeSlotProvisionally  source                                         -            -        yes            0            yes
Return                       register                                       -            -        no             0            no
ImplicitReturn               register                                       -            -        no             0            no
NonLocalReturn               register                                       -            -        no             0            no
SignalNonBoolean             register                                       -            -        no             0            no
Jump                         -                                              -            target   no             0            no
JumpIfNotBoolean             register                                       -            target   yes            0            no
BranchIfFalse                register                                       -            target   yes            0            no
BranchIfTrue                 register                                       -            target   yes            0            no
IntegerBinary                left,right                                     destination  target   yes            0            no
AllocateInstance             -                                              destination  -        yes            0            no
InitializeInstance           instance                                       destination  -        yes            0            no
CheckAppliedClause           -                                              -            -        yes            0            no
InitializeSuperclass         argument_sources                               destination  -        yes            0            no
InitializeAppliedLevel       argument_sources                               destination  -        yes            0            no
```

## Instructions

Each instruction has a page in the frame of the JVM specification's
chapter 6: its operands, its encodings -- its tag in the wire encoding
and its eight-byte forms -- what it reads, writes and jumps to (as the
[register effects](#r-bytecode.register-effects) give it), its effect in
numbered steps, and the errors it signals. A *register* operand is a
register number of the executing frame, a *depth* a number of steps out,
an *index* an instruction index of the body. "*r* := *v*" writes the
value *v* into register *r*.

### LoadLiteral

r[bytecode.instruction.load-literal]
**Operands.** `destination`, a register; `literal`, one of `Nil`,
`Boolean`, `SmallInteger`, `Float`, `String`, `Symbol`, `LargeInteger`,
and the three a body defers to its run, `UnresolvedIdentifier`,
`BodyTooLarge` and `Unbuilt`.
**Encoding.** Wire tag 0; eight-byte forms `LoadNil`, `LoadTrue`,
`LoadFalse`, `LoadSmallInteger`, `LoadConstant`, `LoadLiteral`, `Wide`.
**Reads** nothing. **Writes** `destination`, but for `BodyTooLarge`.
**Jumps** nowhere; falls through, but for `BodyTooLarge`.

1. If *literal* is `UnresolvedIdentifier` naming *n*, signal
   ``internal: unresolved identifier `n` ``. A resolved module carries
   one only as a module root's clause receiver, which never runs
   ([`semantics.module.root-rule`](language/semantics.md#r-semantics.module.root-rule)).
2. If *literal* is `BodyTooLarge` naming a limit, signal `b too large:
   reason`, where *b* is `method`, `block`, `initializer`, `factory` or
   `superclass clause`, the body's kind, and *reason* is `it needs more
   than 65535 registers`, `it takes or sends more than 255 arguments` or
   `it needs more than 1048576 instructions`
   ([`bytecode.limit.too-large`](#r-bytecode.limit.too-large)).
3. If *literal* is `Unbuilt` for an asynchronous send of selector *s*,
   signal ``asynchronous send of `s` needs actors, which ns-next does not
   build yet (§5.6)``
   ([`semantics.gap.actors`](language/semantics.md#r-semantics.gap.actors));
   if it is `Unbuilt` for a pattern literal, signal `a pattern literal
   needs pattern matching, which ns-next does not build yet (§5.1.9)`
   ([`semantics.gap.pattern`](language/semantics.md#r-semantics.gap.pattern)).
4. Otherwise `destination` := the literal's value: `nil`, `true`,
   `false`, the `SmallInteger` or the `Float`; a new `String` each time it
   runs; the `Symbol` of the text; a new `LargeInteger` each time it runs,
   for an integer beyond a `SmallInteger`'s range
   ([`semantics.literal.integer`](language/semantics.md#r-semantics.literal.integer),
   [`semantics.literal.string`](language/semantics.md#r-semantics.literal.string),
   [`semantics.literal.symbol`](language/semantics.md#r-semantics.literal.symbol)).

### LoadSelf

r[bytecode.instruction.load-self]
**Operands.** `destination`, a register.
**Encoding.** Wire tag 1; eight-byte forms `LoadSelf`, `Wide`.
**Reads** nothing. **Writes** `destination`. **Jumps** nowhere.

1. `destination` := the frame's receiver.

### LoadEnclosingObject

r[bytecode.instruction.load-enclosing-object]
**Operands.** `destination`, a register; `depth`, a depth.
**Encoding.** Wire tag 35; eight-byte forms `LoadEnclosingObject`,
`Wide`.
**Reads** nothing. **Writes** `destination`. **Jumps** nowhere.

`outer N` with no message after it
([`semantics.outer.alone`](language/semantics.md#r-semantics.outer.alone)).

1. `destination` := [Outer](runtime.md#r-runtime.outer) applied to the
   running code and `depth`: the enclosing object `depth` levels out from
   the class that declares the running code.

### Move

r[bytecode.instruction.move]
**Operands.** `destination` and `source`, registers.
**Encoding.** Wire tag 3; eight-byte forms `Move`, `Wide`.
**Reads** `source`. **Writes** `destination`. **Jumps** nowhere.

1. `destination` := the value of `source`.

### StoreLocal

r[bytecode.instruction.store-local]
**Operands.** `source`, a register; `slot`, the register of a named
local of the executing frame.
**Encoding.** Wire tag 4; eight-byte forms `StoreLocal`, `Wide`.
**Reads** `source`. **Writes** `slot`. **Jumps** nowhere.

An assignment to a local; reading one needs no instruction, since its
register is its value.

1. `slot` := the value of `source`.

### LoadOuterLocal

r[bytecode.instruction.load-outer-local]
**Operands.** `destination`, a register; `depth`, a depth; `index`, a
register number of the frame `depth` steps out.
**Encoding.** Wire tag 5; eight-byte forms `LoadOuterLocal`, `Wide`.
**Reads** no register of its own frame. **Writes** `destination`.
**Outer reach** `depth`. **Jumps** nowhere.

1. Let *f* be the executing frame. Repeat `depth` times: let *f* be the
   frame that made the closure *f* runs.
2. `destination` := the value of register `index` of *f*.

### StoreOuterLocal

r[bytecode.instruction.store-outer-local]
**Operands.** `source`, a register; `depth` and `index` as
`LoadOuterLocal`'s.
**Encoding.** Wire tag 6; eight-byte forms `StoreOuterLocal`, `Wide`.
**Reads** `source`. **Writes** no register of its own frame. **Outer
reach** `depth`. **Jumps** nowhere.

1. Let *f* be the frame `LoadOuterLocal`'s step 1 finds.
2. Register `index` of *f* := the value of `source`.

### MakeClosure

r[bytecode.instruction.make-closure]
**Operands.** `destination`, a register; `code`, a body of kind `Block`;
`parameters`, the block's parameter names, as many as `code`'s argument
count; `copied`, registers, as many as `code`'s copied count.
**Encoding.** Wire tag 29; eight-byte forms `MakeClosure`, `Wide`.
**Reads** `copied`. **Writes** `destination`. **Outer reach** `code`'s
outer reach less one. **Jumps** nowhere.

1. Let *c* be a new closure of `code` whose making frame is the executing
   frame and whose copied values are the values of `copied`, in order
   ([`semantics.block.closure`](language/semantics.md#r-semantics.block.closure)).
2. If `code`'s outer reach is not 0, the executing frame keeps its
   registers after it returns, for *c* and the closures it makes to
   reach.
3. `destination` := *c*.

An activation of *c* has the receiver and the current class of its making
frame, its parameters in its argument registers and its copied values in
the registers after its header.

### MakeArray

r[bytecode.instruction.make-array]
**Operands.** `destination`, a register; `element_registers`, registers.
**Encoding.** Wire tag 12; eight-byte forms `MakeArray`, `Wide`.
**Reads** `element_registers`. **Writes** `destination`. **Jumps**
nowhere.

A tuple literal `{e1. e2}`, whose elements are evaluated first
([`semantics.gap.immutable-array`](language/semantics.md#r-semantics.gap.immutable-array)).

1. `destination` := a new `Array` whose elements are the values of
   `element_registers`, in order.

### MakeVector

r[bytecode.instruction.make-vector]
**Operands.** `destination`, a register; `size`, a count.
**Encoding.** Wire tag 30; eight-byte forms `MakeVector`, `Wide`.
**Reads** nothing. **Writes** `destination`. **Jumps** nowhere.

A body's *indirection vector*: the `Array` the variables live in that a
block it makes writes, so that the body and every such block share them.
A body has at most one, made before its first statement.

1. `destination` := a new `Array` of `size` elements, each `nil`.

### LoadVector

r[bytecode.instruction.load-vector]
**Operands.** `destination` and `vector`, registers; `index`, an element
index.
**Encoding.** Wire tag 31; eight-byte forms `LoadVector`, `Wide`.
**Reads** `vector`. **Writes** `destination`. **Jumps** nowhere.

1. If `vector` holds no `Array` with an element `index`, counted from 0,
   signal `internal: an indirection vector is not the Array its code
   expects`.
2. `destination` := that element.

### StoreVector

r[bytecode.instruction.store-vector]
**Operands.** `vector` and `source`, registers; `index`, an element
index.
**Encoding.** Wire tag 32; eight-byte forms `StoreVector`, `Wide`.
**Reads** `vector`, `source`. **Writes** no register. **Jumps** nowhere.

1. If `vector` holds no `Array` with an element `index`, signal as
   `LoadVector` does.
2. Element `index` of that `Array` := the value of `source`.

### InitializeSlot

r[bytecode.instruction.initialize-slot]
**Operands.** `source`, a register; `own_index`, the slot's position
among the slots its mixin declares, in declaration order.
**Encoding.** Wire tag 13; eight-byte forms `InitializeSlot`, `Wide`.
**Reads** `source`. **Writes** no register. **Own slots** yes. **Jumps**
nowhere.

A slot's initial value, in an initializer or a lazy slot's getter. The
slot is the receiver's field at the frame's *slot base* -- the first
field of the level of the class the frame was entered at -- plus
`own_index`, so that one body serves every application of its mixin.

1. If the receiver no longer has the slots of that level, signal as
   [`runtime.slot.guard`](runtime.md#r-runtime.slot.guard) says.
2. Let *v* be the value of `source`. If the receiver is a value object,
   *v* is first resolved when it is a future, and if *v* is not a value,
   signal `cannot store v in C's slot s: a value object is deeply
   immutable, so every slot it declares holds a value`
   ([`semantics.value`](language/semantics.md#r-semantics.value)).
3. The slot := *v*.

### InitializeSlotProvisionally

r[bytecode.instruction.initialize-slot-provisionally]
**Operands.** As `InitializeSlot`'s.
**Encoding.** Wire tag 39; eight-byte forms
`InitializeSlotProvisionally`, `Wide`.
**Reads** `source`. **Writes** no register. **Own slots** yes. **Jumps**
nowhere.

The future a simultaneous slot clause stores first, which the clause's
end replaces with its value by an `InitializeSlot`
([`semantics.slot.simultaneous`](language/semantics.md#r-semantics.slot.simultaneous)).

1. As `InitializeSlot`'s step 1.
2. The slot := the value of `source`, unchecked.

### LoadOwnSlot

r[bytecode.instruction.load-own-slot]
**Operands.** `destination`, a register; `own_index` as
`InitializeSlot`'s.
**Encoding.** Wire tag 38; eight-byte forms `LoadOwnSlot`, `Wide`.
**Reads** nothing. **Writes** `destination`. **Own slots** yes.
**Jumps** nowhere.

A simultaneous clause's end reads each slot back without a getter send,
which a subclass could override.

1. As `InitializeSlot`'s step 1.
2. `destination` := the slot's value.

### ReturnSlotIfFilled

r[bytecode.instruction.return-slot-if-filled]
**Operands.** `own_index` as `InitializeSlot`'s.
**Encoding.** Wire tag 34; eight-byte forms `ReturnSlotIfFilled`, `Wide`.
**Reads** nothing. **Writes** no register. **Own slots** yes. **Jumps**
nowhere; falls through when the slot is empty.

A lazy slot's getter's first instruction
([`semantics.slot.lazy`](language/semantics.md#r-semantics.slot.lazy)).

1. As `InitializeSlot`'s step 1.
2. If the slot holds anything but `nil`, return its value from the
   method, as `Return` does.
3. Otherwise go on with the next instruction, the slot's initializer.

### MakeFuture

r[bytecode.instruction.make-future]
**Operands.** `destination` and `closure`, registers.
**Encoding.** Wire tag 36; eight-byte forms `MakeFuture`, `Wide`.
**Reads** `closure`. **Writes** `destination`. **Jumps** nowhere.

1. `destination` := a new unresolved future whose computation is the
   closure in `closure`
   ([`semantics.slot.simultaneous`](language/semantics.md#r-semantics.slot.simultaneous)).

### ResolveFuture

r[bytecode.instruction.resolve-future]
**Operands.** `destination` and `source`, registers.
**Encoding.** Wire tag 37; eight-byte forms `ResolveFuture`, `Wide`.
**Reads** `source`. **Writes** `destination`. **Jumps** nowhere.

A simultaneous clause's end.

1. Let *v* be the value of `source`.
2. If *v* is not a future, `destination` := *v*.
3. If *v* is a resolved future, `destination` := its value.
4. Otherwise `destination` := the answer of
   [Send](runtime.md#r-runtime.send)(*v*, `resolve`, ()), which computes
   the future's value and signals `divergent evaluation: a future was
   forced while it was being computed` for a future already being
   computed.
