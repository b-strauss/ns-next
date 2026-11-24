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
body runs, signalling an error that names the limit.

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
