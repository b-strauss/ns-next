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
range of instruction indices the statement compiled to. For a method or
a block the statements are the initializers of its local variables that
have one, in declaration order, then the statements of its code body.
For an initializer they are, in order: one statement of the checks of
its `<:` clauses, when it has any; one of its superclass initialization;
one per level a `<:` clause adds; one per slot that has an initializer,
in declaration order; then each initializer expression. A lazy slot's
getter has one statement, its initializer, after the
`ReturnSlotIfFilled` it begins with. A frame stands at a statement
boundary when its instruction index is the start of a range.

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
callee is dead. When an instruction with a window executes -- a send but
`SendMirror`, `InitializeSuperclass`, `InitializeAppliedLevel` -- every
register from the end of its window, or from the local count when that
is higher, is dead: the callee's header and registers lie over them. No instruction
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
CompleteInstance             instance                                       -            -        yes            0            no
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
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 0; [eight-byte](#r-bytecode.compact.forms) forms `LoadNil`, `LoadTrue`,
`LoadFalse`, `LoadSmallInteger`, `LoadConstant`, `LoadLiteral`, `Wide`.
**Reads** nothing. **Writes** `destination`, but for `BodyTooLarge`.
**Jumps** nowhere; falls through, but for `BodyTooLarge`.

1. If *literal* is `UnresolvedIdentifier` naming *n*, signal
   ``internal: unresolved identifier `n` ``. A resolved module carries
   one only as a module root's clause receiver, which never runs
   ([`semantics.module.root-rule`](language/semantics.md#r-semantics.module.root-rule)).
2. If *literal* is `BodyTooLarge` naming a limit, signal `b too large:
   reason`, where *b* is `method`, `block`, `initializer`, `factory` or
   `superclass clause`, the kind of the activation running the body, and *reason* is `it needs more
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
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 1; [eight-byte](#r-bytecode.compact.forms) forms `LoadSelf`, `Wide`.
**Reads** nothing. **Writes** `destination`. **Jumps** nowhere.

1. `destination` := the frame's receiver.

### LoadEnclosingObject

r[bytecode.instruction.load-enclosing-object]
**Operands.** `destination`, a register; `depth`, a depth.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 35; [eight-byte](#r-bytecode.compact.forms) forms `LoadEnclosingObject`,
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
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 3; [eight-byte](#r-bytecode.compact.forms) forms `Move`, `Wide`.
**Reads** `source`. **Writes** `destination`. **Jumps** nowhere.

1. `destination` := the value of `source`.

### StoreLocal

r[bytecode.instruction.store-local]
**Operands.** `source`, a register; `slot`, the register of a named
local of the executing frame.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 4; [eight-byte](#r-bytecode.compact.forms) forms `StoreLocal`, `Wide`.
**Reads** `source`. **Writes** `slot`. **Jumps** nowhere.

An assignment to a local; reading one needs no instruction, since its
register is its value.

1. `slot` := the value of `source`.

### LoadOuterLocal

r[bytecode.instruction.load-outer-local]
**Operands.** `destination`, a register; `depth`, a depth; `index`, a
register number of the frame `depth` steps out.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 5; [eight-byte](#r-bytecode.compact.forms) forms `LoadOuterLocal`, `Wide`.
**Reads** no register of its own frame. **Writes** `destination`.
**Outer reach** `depth`. **Jumps** nowhere.

1. Let *f* be the executing frame. Repeat `depth` times: if *f* runs a
   closure, let *f* be the frame that made it; otherwise stop, so that a
   depth past the nesting finds the outermost frame.
2. `destination` := the value of register `index` of *f*.

### StoreOuterLocal

r[bytecode.instruction.store-outer-local]
**Operands.** `source`, a register; `depth` and `index` as
`LoadOuterLocal`'s.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 6; [eight-byte](#r-bytecode.compact.forms) forms `StoreOuterLocal`, `Wide`.
**Reads** `source`. **Writes** no register of its own frame. **Outer
reach** `depth`. **Jumps** nowhere.

1. Let *f* be the frame `LoadOuterLocal`'s step 1 finds.
2. Register `index` of *f* := the value of `source`.

### MakeClosure

r[bytecode.instruction.make-closure]
**Operands.** `destination`, a register; `code`, a body of kind `Block`;
`parameters`, the block's parameter names, as many as `code`'s argument
count; `copied`, registers, as many as `code`'s copied count.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 29; [eight-byte](#r-bytecode.compact.forms) forms `MakeClosure`, `Wide`.
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
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 12; [eight-byte](#r-bytecode.compact.forms) forms `MakeArray`, `Wide`.
**Reads** `element_registers`. **Writes** `destination`. **Jumps**
nowhere.

A tuple literal `{e1. e2}`, whose elements are evaluated first
([`semantics.gap.immutable-array`](language/semantics.md#r-semantics.gap.immutable-array)).

1. `destination` := a new `Array` whose elements are the values of
   `element_registers`, in order.

### MakeVector

r[bytecode.instruction.make-vector]
**Operands.** `destination`, a register; `size`, a count.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 30; [eight-byte](#r-bytecode.compact.forms) forms `MakeVector`, `Wide`.
**Reads** nothing. **Writes** `destination`. **Jumps** nowhere.

A body's *indirection vector*: the `Array` the variables live in that a
block it makes writes, so that the body and every such block share them.
A body has at most one, made before its first statement.

1. `destination` := a new `Array` of `size` elements, each `nil`.

### LoadVector

r[bytecode.instruction.load-vector]
**Operands.** `destination` and `vector`, registers; `index`, an element
index.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 31; [eight-byte](#r-bytecode.compact.forms) forms `LoadVector`, `Wide`.
**Reads** `vector`. **Writes** `destination`. **Jumps** nowhere.

1. If `vector` holds no `Array` with an element `index`, counted from 0,
   signal `internal: an indirection vector is not the Array its code
   expects`.
2. `destination` := that element.

### StoreVector

r[bytecode.instruction.store-vector]
**Operands.** `vector` and `source`, registers; `index`, an element
index.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 32; [eight-byte](#r-bytecode.compact.forms) forms `StoreVector`, `Wide`.
**Reads** `vector`, `source`. **Writes** no register. **Jumps** nowhere.

1. If `vector` holds no `Array` with an element `index`, signal as
   `LoadVector` does.
2. Element `index` of that `Array` := the value of `source`.

### InitializeSlot

r[bytecode.instruction.initialize-slot]
**Operands.** `source`, a register; `own_index`, the slot's position
among the slots its mixin declares, in declaration order.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 13; [eight-byte](#r-bytecode.compact.forms) forms `InitializeSlot`, `Wide`.
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
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 39; [eight-byte](#r-bytecode.compact.forms) forms
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
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 38; [eight-byte](#r-bytecode.compact.forms) forms `LoadOwnSlot`, `Wide`.
**Reads** nothing. **Writes** `destination`. **Own slots** yes.
**Jumps** nowhere.

A simultaneous clause's end reads each slot back without a getter send,
which a subclass could override.

1. As `InitializeSlot`'s step 1.
2. `destination` := the slot's value.

### ReturnSlotIfFilled

r[bytecode.instruction.return-slot-if-filled]
**Operands.** `own_index` as `InitializeSlot`'s.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 34; [eight-byte](#r-bytecode.compact.forms) forms `ReturnSlotIfFilled`, `Wide`.
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
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 36; [eight-byte](#r-bytecode.compact.forms) forms `MakeFuture`, `Wide`.
**Reads** `closure`. **Writes** `destination`. **Jumps** nowhere.

1. If `closure` holds no closure, signal `MakeFuture needs a block`;
   only an image can make it so.
2. `destination` := a new unresolved future whose computation is the
   closure in `closure`
   ([`semantics.slot.simultaneous`](language/semantics.md#r-semantics.slot.simultaneous)).

### ResolveFuture

r[bytecode.instruction.resolve-future]
**Operands.** `destination` and `source`, registers.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 37; [eight-byte](#r-bytecode.compact.forms) forms `ResolveFuture`, `Wide`.
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

### SendPublic

r[bytecode.instruction.send-public]
**Operands.** `selector`, a symbol; `receiver_register`, a register;
`argument_base` and `argument_sources`, the send's window
([`bytecode.register.argument-window`](#r-bytecode.register.argument-window));
`destination`, a register; `call_site`, its call-site number
([`bytecode.call-site`](#r-bytecode.call-site)).
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 47; [eight-byte](#r-bytecode.compact.forms) forms `SendPublic0`, `SendPublic1`,
`SendPublic2`, `SendPublicWide`.
**Reads** `receiver_register`, `argument_sources`. **Writes**
`destination`. **Jumps** nowhere.

An ordinary send to an explicit receiver
([`semantics.send.ordinary`](language/semantics.md#r-semantics.send.ordinary)).

1. Copy each source into its slot of the window.
2. `destination` := [Send](runtime.md#r-runtime.send)(the value of
   `receiver_register`, `selector`, the values of the window).

### SendInlinable

r[bytecode.instruction.send-inlinable]
**Operands.** As `SendPublic`'s.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 48; [eight-byte](#r-bytecode.compact.forms) forms `SendInlinable0`,
`SendInlinable1`, `SendInlinable2`, `SendInlinableWide`.
**Reads** `receiver_register`, `argument_sources`. **Writes**
`destination`. **Jumps** nowhere.

Exactly `SendPublic`, emitted only by the reference lowering, for the
sends the optimized lowering inlines as control flow (`ifTrue:`,
`whileTrue:`, `and:` and their kin). It differs in one respect: a block
it activates for the control-flow step is not counted against the depth
limit ([`runtime.depth-limit`](runtime.md#r-runtime.depth-limit)),
since the optimized lowering never activates that block, so both
lowerings meet the limit at the same depth.

1. As `SendPublic`'s steps.

### SendLexical

r[bytecode.instruction.send-lexical]
**Operands.** `depth`, a depth; `selector`, `argument_base`,
`argument_sources`, `destination` and `call_site` as `SendPublic`'s.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 46; [eight-byte](#r-bytecode.compact.forms) forms `SendLexical0`,
`SendLexical1`, `SendLexical2`, `SendLexicalWide`.
**Reads** `argument_sources`. **Writes** `destination`. **Jumps**
nowhere.

A send whose receiver the resolver decided from the lexical scope: an
implicit-receiver send that a declaration `depth` levels out answers, or
`outer N m`
([`semantics.send.implicit-receiver`](language/semantics.md#r-semantics.send.implicit-receiver),
[`semantics.send.outer`](language/semantics.md#r-semantics.send.outer)).

1. Copy each source into its slot of the window.
2. Let *r* be the frame's receiver when `depth` is 0, and otherwise
   [Outer](runtime.md#r-runtime.outer) applied to the running code and
   `depth`.
3. `destination` := the answer of the send of `selector` to *r* with the
   values of the window, its method found as
   [ImplicitReceiverLookup](runtime.md#r-runtime.implicit-receiver) and
   [OuterSend](runtime.md#r-runtime.outer-send) find it: the private and
   protected members of the declaring class included.

### SendSuper

r[bytecode.instruction.send-super]
**Operands.** `selector`, `argument_base`, `argument_sources`,
`destination` and `call_site` as `SendPublic`'s.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 49; [eight-byte](#r-bytecode.compact.forms) forms `SendSuper0`, `SendSuper1`,
`SendSuper2`, `SendSuperWide`.
**Reads** `argument_sources`. **Writes** `destination`. **Jumps**
nowhere.

1. Copy each source into its slot of the window.
2. `destination` := the answer of the super send of `selector` with the
   values of the window, as [Send](runtime.md#r-runtime.send) gives it:
   its receiver the frame's, its lookup starting at the superclass of the
   current class
   ([`semantics.send.super`](language/semantics.md#r-semantics.send.super)).

### SendMirror

r[bytecode.instruction.send-mirror]
**Operands.** `primitive`, a row of the VM mirror's table;
`mirror_register` and `subject_register`, registers; `argument_base` and
`argument_sources`, its window; `destination`, a register; `failure`, an
index.
**Encoding.** No wire tag: it is emitted only into the shipped core
library's code, which no image carries
([`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror)). [Eight-byte](#r-bytecode.compact.forms) forms
`SendMirror`, `Wide`.
**Reads** `mirror_register`, `subject_register`, `argument_sources`.
**Writes** `destination`. **Jumps** to `failure`.

A send to the VM mirror whose failure block was compiled inline at
`failure`. The row is decided when the code is compiled; the check that
`mirror_register` holds the mirror stands for the lookup.

1. Copy each source into its slot of the window.
2. If `mirror_register` does not hold the VM mirror, signal `internal: a
   mirror send's receiver is not the VM mirror`.
3. Run the row's operation on the value of `subject_register` and the
   values of the window. If it answers a value, `destination` := that
   value. If it declines its operands, go to `failure`.

### Return

r[bytecode.instruction.return]
**Operands.** `register`, a register.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 14; [eight-byte](#r-bytecode.compact.forms) forms `Return`, `Wide`.
**Reads** `register`. **Writes** nothing. **Jumps** nowhere; never falls
through.

A `^` outside a block
([`semantics.method.return`](language/semantics.md#r-semantics.method.return)).

1. End the frame, and make the instruction that made it answer the value
   of `register`.

### ImplicitReturn

r[bytecode.instruction.implicit-return]
**Operands.** `register`, a register.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 15; [eight-byte](#r-bytecode.compact.forms) forms `ImplicitReturn`, `Wide`.
**Reads** `register`. **Writes** nothing. **Jumps** nowhere; never falls
through.

A body that fell off its end: a method answers `self`, a block its last
expression's value. It runs exactly as `Return` does.

1. As `Return`'s step.

### NonLocalReturn

r[bytecode.instruction.non-local-return]
**Operands.** `register`, a register.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 16; [eight-byte](#r-bytecode.compact.forms) forms `NonLocalReturn`, `Wide`.
**Reads** `register`. **Writes** nothing. **Jumps** nowhere; never falls
through.

A `^` inside a block
([`semantics.block.non-local-return`](language/semantics.md#r-semantics.block.non-local-return)).

1. Let *h* be the *home* of the frame: the method frame the block's
   outermost making frame runs in.
2. If *h* has already returned, signal as
   [`semantics.block.cannot-return`](language/semantics.md#r-semantics.block.cannot-return)
   says.
3. [Unwind](runtime.md#r-runtime.unwind)(*h*, the value of `register`).

### SignalNonBoolean

r[bytecode.instruction.signal-non-boolean]
**Operands.** `register`, a register.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 40; [eight-byte](#r-bytecode.compact.forms) forms `SignalNonBoolean`, `Wide`.
**Reads** `register`. **Writes** nothing. **Jumps** nowhere; never falls
through.

The guard-failure path of an inlined `whileTrue:` or `whileFalse:`.

1. Signal a `NonBooleanReceiver` with the text `a loop's condition
   answered v, which is no Boolean`, *v* the value of `register`
   described. Nothing after it runs.

### Jump

r[bytecode.instruction.jump]
**Operands.** `target`, an index.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 17; [eight-byte](#r-bytecode.compact.forms) forms `Jump`, `Wide`.
**Reads** nothing. **Writes** nothing. **Jumps** to `target`; never
falls through.

1. Go to `target`.

### JumpIfNotBoolean

r[bytecode.instruction.jump-if-not-boolean]
**Operands.** `register`, a register; `target`, an index.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 18; [eight-byte](#r-bytecode.compact.forms) forms `JumpIfNotBoolean`, `Wide`.
**Reads** `register`. **Writes** nothing. **Jumps** to `target`.

The guard in front of inlined control flow: its target sends the message
the inlined code stands for to a receiver that is no Boolean.

1. If `register` holds neither `true` nor `false`, go to `target`.

### BranchIfFalse

r[bytecode.instruction.branch-if-false]
**Operands.** `register`, a register; `target`, an index.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 19; [eight-byte](#r-bytecode.compact.forms) forms `BranchIfFalse`, `Wide`.
**Reads** `register`. **Writes** nothing. **Jumps** to `target`.

1. If `register` holds `false`, go to `target`.

### BranchIfTrue

r[bytecode.instruction.branch-if-true]
**Operands.** `register`, a register; `target`, an index.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 20; [eight-byte](#r-bytecode.compact.forms) forms `BranchIfTrue`, `Wide`.
**Reads** `register`. **Writes** nothing. **Jumps** to `target`.

1. If `register` holds `true`, go to `target`.

### IntegerBinary

r[bytecode.instruction.integer-binary]
**Operands.** `operation`, one of `Add` (`+`), `Subtract` (`-`),
`Multiply` (`*`), `FloorDivide` (`//`), `Less` (`<`), `Greater` (`>`),
`LessOrEqual` (`<=`), `GreaterOrEqual` (`>=`) and `Equal` (`=`);
`destination`, `left` and `right`, registers; `target`, an index.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 33; [eight-byte](#r-bytecode.compact.forms) forms `IntegerAdd`,
`IntegerSubtract`, `IntegerMultiply`, `IntegerFloorDivide`,
`IntegerLess`, `IntegerGreater`, `IntegerLessOrEqual`,
`IntegerGreaterOrEqual`, `IntegerEqual`, `Wide`.
**Reads** `left`, `right`. **Writes** `destination`. **Jumps** to
`target`.

A binary send of the operation's selector, computed inline; the ordinary
send stands at `target`, so the two answer alike.

1. If `left` and `right` both hold a `SmallInteger`, and the operation
   on them has a result that is a `SmallInteger` or a Boolean --
   no divisor 0, no result outside a `SmallInteger`'s range --
   `destination` := that result.
2. Otherwise go to `target`.

### AllocateInstance

r[bytecode.instruction.allocate-instance]
**Operands.** `destination`, a register.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 41; [eight-byte](#r-bytecode.compact.forms) forms `AllocateInstance`, `Wide`.
**Reads** nothing. **Writes** `destination`. **Jumps** nowhere.

A factory body's allocation; the frame's receiver is the class the
factory was sent to.

1. `destination` := a new instance of that class, every slot `nil`, as
   [Instantiate](runtime.md#r-runtime.instantiate)'s allocation makes
   it, signalling what that step signals for a class that refuses one.

### InitializeInstance

r[bytecode.instruction.initialize-instance]
**Operands.** `instance` and `destination`, registers.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 42; [eight-byte](#r-bytecode.compact.forms) forms `InitializeInstance`, `Wide`.
**Reads** `instance`. **Writes** `destination`. **Jumps** nowhere.

A factory body's initialization, with the factory's arguments -- the
frame's own argument registers, which it names by no operand.

1. `destination` := the answer of the initializer of the frame's receiver
   class run on the value of `instance` with the frame's arguments, as
   [Instantiate](runtime.md#r-runtime.instantiate) runs it
   ([`semantics.instantiation.initializer-order`](language/semantics.md#r-semantics.instantiation.initializer-order)).

### CompleteInstance

r[bytecode.instruction.complete-instance]
**Operands.** `instance`, a register.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 52; [eight-byte](#r-bytecode.compact.forms) forms `CompleteInstance`, `Wide`.
**Reads** `instance`. **Writes** nothing. **Jumps** nowhere.

A factory body's completion, after its initialization returned.

1. If `instance` holds an ordinary instance marked under construction,
   clear the mark, as [Instantiate](runtime.md#r-runtime.instantiate)'s
   last step does. Any other value -- an unmarked instance, an object of
   another kind, a value that is no object -- is left as it is.

It signals nothing.

### CheckAppliedClause

r[bytecode.instruction.check-applied-clause]
**Operands.** `steps`, a number of levels; `selector`, a symbol.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 43; [eight-byte](#r-bytecode.compact.forms) forms `CheckAppliedClause`, `Wide`.
**Reads** nothing. **Writes** nothing. **Jumps** nowhere.

An initializer's check of a `<:` clause, before anything of it runs
(Bracha §6.1).

1. Let *L* be the level `steps` above the frame's class, the one the
   clause added. If its primary factory's selector is not `selector`,
   signal `a mixin application clause of class A sends #selector, but
   the primary factory of class C is #f`.

### InitializeSuperclass

r[bytecode.instruction.initialize-superclass]
**Operands.** `steps`, a number of levels; `selector`, a symbol;
`argument_base`, `argument_sources` and `destination` as a send's.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 50; [eight-byte](#r-bytecode.compact.forms) forms `InitializeSuperclass`,
`Wide`.
**Reads** `argument_sources`. **Writes** `destination`. **Jumps**
nowhere.

An initializer's superclass initialization (Bracha §6.3).

1. Copy each source into its slot of the window.
2. Let *S* be the class `steps` above the frame's class, the one the
   superclass clause names. If its primary factory's selector is not
   `selector`, signal `the superclass clause of class A sends #selector,
   but the primary factory of class S is #f`.
3. `destination` := the answer of *S*'s initializer run on the frame's
   receiver with the values of the window. A chain that ends before
   `steps` levels has nothing to initialize: `destination` := the
   receiver.

### InitializeAppliedLevel

r[bytecode.instruction.initialize-applied-level]
**Operands.** `steps`, a number of levels; `argument_base`,
`argument_sources` and `destination` as a send's.
**Encoding.** [Wire](#r-bytecode.wire.tags) tag 51; [eight-byte](#r-bytecode.compact.forms) forms `InitializeAppliedLevel`,
`Wide`.
**Reads** `argument_sources`. **Writes** `destination`. **Jumps**
nowhere.

The initialization of a level a `<:` clause adds, run just before that
level's own part, with the applying class's factory arguments already
evaluated into the window
([`semantics.mixin.application`](language/semantics.md#r-semantics.mixin.application),
[`runtime.apply-mixin`](runtime.md#r-runtime.apply-mixin)).

1. Copy each source into its slot of the window.
2. `destination` := the answer of the own part of the initializer of the
   mixin of the level `steps` above the frame's class, run on the frame's
   receiver with the values of the window.

## The wire encoding

The form in which an image carries a body
([Image format](image-format.md)). Every number is little-endian; a
*symbol* is a `u32` id into the image's symbol table; *bytes* are a `u32`
length and that many bytes of UTF-8.

r[bytecode.wire.body]
A body is written as, in this order: its kind's tag (`u8`); its argument
count, register count, copied count and local count (each `u16`); its
source span's start and end (each `u32`); its statement map, a `u32`
count of entries, each a statement's index and its range's start and end
(each `u32`); whether its sends are uncached (`u8`, 0 or 1); its
instruction count (`u32`); and its instructions. Its outer reach, whether
it addresses its own slots and its family size are not written: the
decoder derives them from the instructions, as the compiler does.

The fields of a body's record, in order, each with its width;
`statements` is the statement map, a `u32` count and that many entries of
three `u32`s, and `instructions` the instructions themselves:

```spec-table wire-body
kind:u8  argument_count:u16  register_count:u16  copied_count:u16  local_count:u16  source_start:u32  source_end:u32  statement_map:statements  uncached:u8  instruction_count:u32  instructions:instructions
```

r[bytecode.wire.instruction]
An instruction is written as its tag (`u8`) followed by its operands, in
the order and the widths its row of the layout table gives: `u8`, `u16`
or `u32`; `symbol`; `registers`, a `u16` count and that many `u16`
register numbers; `symbols`, a `u16` count and that many symbols; `code`,
the `u32` id of a body in the image's code table; `literal`, a literal as
`bytecode.wire.literal` writes it. No instruction's operands nest.
`SendMirror` has no wire form.

r[bytecode.wire.literal]
A literal is written as its tag (`u8`) followed by its payload, as its
row of the layout table gives: `u8` 0 or 1 for a Boolean; an `i64` for a
`SmallInteger`, and for a `LargeInteger` whose value fits an `i64`, which
is written as a `SmallInteger`; the `f64`'s bits for a `Float`; `bytes`
for a `String` and an unresolved identifier's name; a `symbol` for a
`Symbol` and an asynchronous send's selector; for any other
`LargeInteger`, `large-integer`: its sign (`u8`, 1 for negative), a `u32`
count of digits and that many `u64` digits, least significant first.

r[bytecode.wire.call-sites]
Call-site numbers are not written. The decoder numbers the sends of each
code family in the order the compiler emitted them, which a family's
bodies record -- or, for a body written uncached, marks every send
uncached ([`bytecode.call-site`](#r-bytecode.call-site)).

r[bytecode.wire.tags]
The tags of the four spaces a body's encoding uses -- instructions,
literals, integer operations and code kinds -- are those of the table;
`retired` marks a number no longer assigned, which the decoder refuses
as it refuses any number the table does not give.

r[bytecode.wire.tag-assigned-once]
A tag is assigned once and never changes meaning: a new instruction,
literal, operation or kind takes the next free number, and a removed one
retires its number rather than freeing it, so that bytes written under an
old assignment meet a refusal rather than decode into another program.

```spec-table wire-tags
instruction        LoadLiteral                  0
instruction        LoadSelf                     1
instruction        retired                      2
instruction        Move                         3
instruction        StoreLocal                   4
instruction        LoadOuterLocal               5
instruction        StoreOuterLocal              6
instruction        retired                      7
instruction        retired                      8
instruction        retired                      9
instruction        retired                      10
instruction        retired                      11
instruction        MakeArray                    12
instruction        InitializeSlot               13
instruction        Return                       14
instruction        ImplicitReturn               15
instruction        NonLocalReturn               16
instruction        Jump                         17
instruction        JumpIfNotBoolean             18
instruction        BranchIfFalse                19
instruction        BranchIfTrue                 20
instruction        retired                      21
instruction        retired                      22
instruction        retired                      23
instruction        retired                      24
instruction        retired                      25
instruction        retired                      26
instruction        retired                      27
instruction        retired                      28
instruction        MakeClosure                  29
instruction        MakeVector                   30
instruction        LoadVector                   31
instruction        StoreVector                  32
instruction        IntegerBinary                33
instruction        ReturnSlotIfFilled           34
instruction        LoadEnclosingObject          35
instruction        MakeFuture                   36
instruction        ResolveFuture                37
instruction        LoadOwnSlot                  38
instruction        InitializeSlotProvisionally  39
instruction        SignalNonBoolean             40
instruction        AllocateInstance             41
instruction        InitializeInstance           42
instruction        CheckAppliedClause           43
instruction        retired                      44
instruction        retired                      45
instruction        SendLexical                  46
instruction        SendPublic                   47
instruction        SendInlinable                48
instruction        SendSuper                    49
instruction        InitializeSuperclass         50
instruction        InitializeAppliedLevel       51
instruction        CompleteInstance             52
literal            Nil                          0
literal            Boolean                      1
literal            SmallInteger                 2
literal            Float                        3
literal            String                       4
literal            Symbol                       5
literal            retired                      6
literal            retired                      7
literal            retired                      8
literal            UnresolvedIdentifier         9
literal            LargeInteger                 10
literal            BodyTooLarge/Registers       11
literal            BodyTooLarge/Arguments       12
literal            BodyTooLarge/Instructions    13
literal            Unbuilt/AsynchronousSend     14
literal            Unbuilt/PatternLiteral       15
integer-operation  Add                          0
integer-operation  Subtract                     1
integer-operation  Multiply                     2
integer-operation  FloorDivide                  3
integer-operation  Less                         4
integer-operation  Greater                      5
integer-operation  LessOrEqual                  6
integer-operation  GreaterOrEqual               7
integer-operation  Equal                        8
code-kind          retired                      0
code-kind          retired                      1
code-kind          retired                      2
code-kind          retired                      3
code-kind          retired                      4
code-kind          Method                       5
code-kind          Block                        6
code-kind          Initializer                  7
code-kind          SuperclassClause             8
```

The layouts, an instruction's or a literal's operands after its tag:

```spec-table wire-layouts
LoadLiteral                  destination:u16  literal:literal
LoadSelf                     destination:u16
LoadEnclosingObject          destination:u16  depth:u16
Move                         destination:u16  source:u16
StoreLocal                   source:u16  slot:u16
LoadOuterLocal               destination:u16  depth:u16  index:u16
StoreOuterLocal              source:u16  depth:u16  index:u16
SendLexical                  depth:u16  selector:symbol  argument_base:u16  argument_sources:registers  destination:u16
SendPublic                   selector:symbol  receiver_register:u16  argument_base:u16  argument_sources:registers  destination:u16
SendInlinable                selector:symbol  receiver_register:u16  argument_base:u16  argument_sources:registers  destination:u16
SendSuper                    selector:symbol  argument_base:u16  argument_sources:registers  destination:u16
MakeClosure                  destination:u16  parameters:symbols  code:code  copied:registers
MakeArray                    destination:u16  element_registers:registers
MakeVector                   destination:u16  size:u16
LoadVector                   destination:u16  vector:u16  index:u16
StoreVector                  vector:u16  index:u16  source:u16
InitializeSlot               source:u16  own_index:u32
ReturnSlotIfFilled           own_index:u32
MakeFuture                   destination:u16  closure:u16
ResolveFuture                destination:u16  source:u16
LoadOwnSlot                  destination:u16  own_index:u32
InitializeSlotProvisionally  source:u16  own_index:u32
Return                       register:u16
ImplicitReturn               register:u16
NonLocalReturn               register:u16
SignalNonBoolean             register:u16
Jump                         target:u32
JumpIfNotBoolean             register:u16  target:u32
BranchIfFalse                register:u16  target:u32
BranchIfTrue                 register:u16  target:u32
IntegerBinary                operation:u8  destination:u16  left:u16  right:u16  target:u32
AllocateInstance             destination:u16
InitializeInstance           instance:u16  destination:u16
CompleteInstance             instance:u16
CheckAppliedClause           steps:u16  selector:symbol
InitializeSuperclass         steps:u16  selector:symbol  argument_base:u16  argument_sources:registers  destination:u16
InitializeAppliedLevel       steps:u16  argument_base:u16  argument_sources:registers  destination:u16
literal/Nil
literal/Boolean              value:u8
literal/SmallInteger         value:i64
literal/Float                value:f64
literal/String               text:bytes
literal/Symbol               symbol:symbol
literal/UnresolvedIdentifier name:bytes
literal/LargeInteger         value:large-integer
literal/BodyTooLarge/Registers
literal/BodyTooLarge/Arguments
literal/BodyTooLarge/Instructions
literal/Unbuilt/AsynchronousSend  selector:symbol
literal/Unbuilt/PatternLiteral
```

## The eight-byte form

The form in which a body is stored and the run loop executes it. It is
not part of an image: the decoder encodes every instruction it reads into
it, and the encoder writes each instruction it reads back out of it.

r[bytecode.compact.slot]
Each instruction is one slot of eight bytes, so that an instruction index
is a slot's index: a jump target, a statement map's range and a frame's
instruction index mean the same in both encodings. A slot is one of the
forms of the table, its first byte the form. In a slot a register is a
`u8`, a jump target an absolute `u32` instruction index, a depth or a
level count a `u16`, and anything that does not fit is an index into one
of the body's side tables.

r[bytecode.compact.tables]
A body's side tables, each indexed from 0 in the order its entries were
added:

- `constants`, the immediates `LoadConstant` loads: a `SmallInteger`
  outside an `i32`'s range, a `Float` that is a `SmallFloat64`;
- `literals`, the literals `LoadLiteral` makes a value of each time it
  runs: a `String`, a `Symbol`, a `LargeInteger`, a `Float` outside the
  `SmallFloat64` range, and the three deferred ones;
- `send_sites`, one per send: its selector, its call site, a
  `SendLexical`'s depth, and for a wide send its destination, receiver,
  window and sources;
- `blocks`, one per `MakeClosure`: its body, its parameters and its copied
  registers;
- `mirror_sites`, one per `SendMirror`: its window, its sources and its
  failure index;
- `superclass_sites`, one per `InitializeSuperclass`: its level count,
  selector, window and sources;
- `register_lists`, a `MakeArray`'s element registers and an
  `InitializeAppliedLevel`'s sources;
- `wide`, the instruction itself for every `Wide` slot.

r[bytecode.compact.choice]
The form an instruction takes is decided by the instruction alone and the
tables' lengths so far:

1. `LoadLiteral` of `nil`, `true` or `false` takes `LoadNil`, `LoadTrue`
   or `LoadFalse`; of a `SmallInteger` in an `i32`'s range,
   `LoadSmallInteger`; of another `SmallInteger` or a `SmallFloat64`,
   `LoadConstant`; of anything else, `LoadLiteral`.
2. A send of no, one or two arguments takes its kind's `0`, `1` or `2`
   form, and of more its `Wide` form, which reads everything from its send
   site.
3. `IntegerBinary` takes the form of its operation, `IntegerAdd` to
   `IntegerEqual`.
4. Every other instruction takes the form of its name.
5. An instruction whose eight-byte form would hold a register above 255,
   or append to a table that already holds 65,536 entries, takes its wide
   form instead: a public, inlinable, lexical or super send its kind's wide
   form, every other instruction `Wide`.

r[bytecode.compact.wide]
A `Wide` slot holds the index of its instruction in `wide`; it does what
its instruction does. Any instruction stored in its wide form behaves as
in its eight-byte form: a stress mode stores every instruction wide, and
every program behaves the same under it.

r[bytecode.compact.breakpoint]
A `Breakpoint` slot is one a debugger's breakpoint patched, the slot it
replaced kept beside the slots. No compiler emits one, no image holds
one, and every reader but the run loop's fetch reads the slot it replaced.

r[bytecode.compact.round-trip]
A body read back from its slots and tables gives the instructions it was
built from, and two bodies are the same when they mean the same: a body
built narrow equals the same body built wide.

r[bytecode.compact.forms]
The forms, with their fields in the order of the slot, each with its
width: `u8x2` is two registers, `symbol` a selector, `primitive` a row of
the VM mirror's table.

```spec-table compact-forms
LoadNil                      destination:u8
LoadTrue                     destination:u8
LoadFalse                    destination:u8
LoadSmallInteger             destination:u8  value:i32
LoadConstant                 destination:u8  constant:u16
LoadLiteral                  destination:u8  literal:u16
LoadSelf                     destination:u8
LoadEnclosingObject          destination:u8  depth:u16
Move                         destination:u8  source:u8
StoreLocal                   source:u8  slot:u8
LoadOuterLocal               destination:u8  index:u8  depth:u16
StoreOuterLocal              source:u8  index:u8  depth:u16
SendPublic0                  destination:u8  receiver:u8  argument_base:u8  site:u16
SendPublic1                  destination:u8  receiver:u8  argument_base:u8  source:u8  site:u16
SendPublic2                  destination:u8  receiver:u8  argument_base:u8  sources:u8x2  site:u16
SendPublicWide               site:u32
SendInlinable0               destination:u8  receiver:u8  argument_base:u8  site:u16
SendInlinable1               destination:u8  receiver:u8  argument_base:u8  source:u8  site:u16
SendInlinable2               destination:u8  receiver:u8  argument_base:u8  sources:u8x2  site:u16
SendInlinableWide            site:u32
SendLexical0                 destination:u8  argument_base:u8  site:u16
SendLexical1                 destination:u8  argument_base:u8  source:u8  site:u16
SendLexical2                 destination:u8  argument_base:u8  sources:u8x2  site:u16
SendLexicalWide              site:u32
SendSuper0                   destination:u8  argument_base:u8  site:u16
SendSuper1                   destination:u8  argument_base:u8  source:u8  site:u16
SendSuper2                   destination:u8  argument_base:u8  sources:u8x2  site:u16
SendSuperWide                site:u32
SendMirror                   destination:u8  mirror:u8  subject:u8  primitive:primitive  site:u16
MakeClosure                  destination:u8  block:u16
MakeArray                    destination:u8  elements:u16
MakeVector                   destination:u8  size:u16
LoadVector                   destination:u8  vector:u8  index:u16
StoreVector                  vector:u8  source:u8  index:u16
InitializeSlot               source:u8  own_index:u32
ReturnSlotIfFilled           own_index:u32
MakeFuture                   destination:u8  closure:u8
ResolveFuture                destination:u8  source:u8
LoadOwnSlot                  destination:u8  own_index:u32
InitializeSlotProvisionally  source:u8  own_index:u32
Return                       register:u8
ImplicitReturn               register:u8
NonLocalReturn               register:u8
SignalNonBoolean             register:u8
Jump                         target:u32
JumpIfNotBoolean             register:u8  target:u32
BranchIfFalse                register:u8  target:u32
BranchIfTrue                 register:u8  target:u32
IntegerAdd                   destination:u8  left:u8  right:u8  target:u32
IntegerSubtract              destination:u8  left:u8  right:u8  target:u32
IntegerMultiply              destination:u8  left:u8  right:u8  target:u32
IntegerFloorDivide           destination:u8  left:u8  right:u8  target:u32
IntegerLess                  destination:u8  left:u8  right:u8  target:u32
IntegerGreater               destination:u8  left:u8  right:u8  target:u32
IntegerLessOrEqual           destination:u8  left:u8  right:u8  target:u32
IntegerGreaterOrEqual        destination:u8  left:u8  right:u8  target:u32
IntegerEqual                 destination:u8  left:u8  right:u8  target:u32
AllocateInstance             destination:u8
InitializeInstance           destination:u8  instance:u8
CompleteInstance             instance:u8
CheckAppliedClause           steps:u16  selector:symbol
InitializeSuperclass         destination:u8  site:u16
InitializeAppliedLevel       destination:u8  argument_base:u8  steps:u16  arguments:u16
Wide                         index:u32
Breakpoint
```

## The rules the decoder checks

An image is untrusted input. The image decoder refuses a body, and with
it the image, unless every rule below holds; it checks them before any
code of the image runs, and no byte sequence makes it fail otherwise than
by refusing.

r[bytecode.verify.tags]
1. Every instruction, literal, integer-operation and code-kind tag is
   one the [wire tags](#r-bytecode.wire.tags) give, never a retired one,
   and every symbol id names an entry of the image's symbol table.

r[bytecode.verify.body-fields]
2. A body's argument count is at most 255 and at most its register count;
   its local count is at most its register count; its copied values fit
   between its header and its register count; its source span does not
   end before it starts; and it holds at most 2^20 instructions
   ([`bytecode.limit.instructions`](#r-bytecode.limit.instructions),
   [`bytecode.limit.arguments`](#r-bytecode.limit.arguments)).

r[bytecode.verify.statement-map]
3. Every range of the statement map lies within the body: its start is
   at most its end, and its end at most the instruction count.

r[bytecode.verify.registers]
4. Every register operand of an instruction is below the body's register
   count and is none of its header's numbers, and no instruction writes
   an argument register
   ([`bytecode.register.layout`](#r-bytecode.register.layout),
   [`bytecode.register.arguments`](#r-bytecode.register.arguments)).

r[bytecode.verify.argument-windows]
5. Every window lies below the register count and starts at or above
   both the header's end and the local count; no source is another slot of
   its window; and no send, superclass clause message or block passes or
   takes more than 255 arguments
   ([`bytecode.register.argument-window`](#r-bytecode.register.argument-window)).

r[bytecode.verify.control-flow]
6. Every jump target is an instruction index of the body, and the body
   holds at least one instruction and ends with `Return`, `ImplicitReturn`,
   `NonLocalReturn`, `SignalNonBoolean` or `Jump`, so that control never
   runs past its end.

r[bytecode.verify.blocks]
7. A `MakeClosure` names a body of kind `Block` in the image's code table
   that comes before the body that names it; no block body is named by
   two of them; its parameter count is the block's argument count and its
   copied registers are as many as the block's copied count; and blocks
   nest at most 128 deep, the parser's own limit.

r[bytecode.verify.outer-locals]
8. An outer local's index, counted in the body `depth` steps out -- the
   outermost body when the depth exceeds the nesting -- is below that
   body's local count, and so below its register count, is none of its
   header's numbers, and, for `StoreOuterLocal`, is not one of its
   arguments.

r[bytecode.verify.leftovers]
9. No instruction reads a dead register
   ([`bytecode.register.leftovers`](#r-bytecode.register.leftovers)).
   The analysis that decides it refuses a body past what any compiled body
   reaches: more than 257 jumps open at once, more than 128 loops open at
   once, two loops that cross, or a jump into a loop past its head.

r[bytecode.verify.large-integer]
10. A `LargeInteger` literal is in its one canonical encoding: its sign
    byte is 0 or 1, its value does not fit an `i64`, its most significant
    digit is not 0, and it is not a negative zero.

r[bytecode.verify.attachment]
11. A body's kind and argument count agree with what it is attached to:
    a method's with its selector's arity, an initializer's with its
    factory's, a superclass clause's with none.

r[bytecode.verify.send-arity]
12. Every `SendLexical`, `SendPublic`, `SendInlinable` and `SendSuper`
    passes as many arguments as its selector takes: none for a unary
    selector, one for a binary selector and one for each colon of a
    keyword selector. The method a send reaches reads the arguments its
    selector takes, so a send of fewer would hand it an argument it was
    not given and a send of more one it never reads; no lowering emits
    either.

r[bytecode.verify.not-checked]
The decoder checks no more of a body than these rules. In particular it
leaves these to the run, where they are checked or cannot go wrong:

- an own-slot instruction's `own_index`, which no body bounds -- the
  decoder holds every instance of a program class to its class's slot
  count instead, so that an index outside them reads no other object;
- a vector's index and size, which `LoadVector` and `StoreVector` check
  as they run;
- a `LoadEnclosingObject`'s and a `SendLexical`'s depth: one past the
  chain of enclosing objects the walk out finds answers `nil`;
- an `InitializeSuperclass`'s argument count, which is checked when it
  runs, against the factory of the class it initializes, before anything
  is entered.

The shipped core library's code, which the binary embeds, is not decoded
by these rules: it is the binary's own, and the binary that built it
trusts it as it trusts its own machine code.
