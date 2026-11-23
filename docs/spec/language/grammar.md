# Grammar

How the tokens of the lexical structure form a module. The grammar is
written over tokens: a quoted `"x"` is the token spelled `x`, a lowercase
name is a production, `[x]` is optional, `empty` derives nothing, and
`identifier`, `keyword`, `setterKeyword`, `binarySelector`, `number`,
`string` and `symbol` are the tokens of
[Lexical structure](lexical-structure.md). `name` is an identifier or one
of the words with tokens of their own
([`syntax.unreserved-word`](#r-syntax.unreserved-word)), and
`unarySelector` is a name.

## How the grammar is read

r[syntax.peg]
The grammar is a parsing expression grammar over tokens. An optional part
`[x]` is taken whenever it matches and is never retried as absent; a
repetition `x*` or `x+` takes as many as match; of alternatives `a | b`
the first that matches in full wins. Where a production could be read two
ways, the production written first is tried in full and the other only
when it fails. A module whose tokens the production `compilationUnit`
does not derive, followed by the end of input, is refused.

r[syntax.whitespace-free]
Whitespace and comments never appear in a production: they only separate
tokens ([`lex.whitespace`](lexical-structure.md#r-lex.whitespace)). A
production that Bracha writes with `whitespace` after a word --
`accessModifier`, `lazyModifier` -- is the word alone, so `| public= 1. |`
and `| public = 1. |` are the same tokens and are refused alike (Bracha
§6.3.3 and §6.3.4; see the appendix).

r[syntax.reserved-word]
A reserved word ([`lex.reserved-words`](lexical-structure.md#r-lex.reserved-words))
may not name a parameter, a block parameter, a slot or a temporary, a
class, a superclass, an enclosing class after `outer` in a superclass
clause, a type parameter or a method, may not be the target of a setter
send (`nil:: 3`), and may not stand as a unary selector (`3 nil`,
`3 printString; self`).
Each such use is refused naming what the word may not be:
``  `nil` is a reserved word and cannot name a parameter (§4.1) ``. A
keyword that merely begins with a reserved word, `ifNil:` or `selfish:`,
is no reserved word.

r[syntax.unreserved-word]
`class`, `public`, `protected`, `private` and `lazy` are names wherever
the grammar takes an identifier -- a name read, written or sent, a
parameter, a block parameter, a class, a superclass clause's receiver,
`outer`'s class name, the language id. Where such a word could also begin
a declaration, [`syntax.peg`](#r-syntax.peg) decides: an access modifier's
word standing first in a slot or a method is the modifier, so it names a
slot, a temporary or a method only behind one -- `| public public = 1. |`
and `public private = ( ^1 )` are accepted, `| public |` and
`public = ( ^1 )` are refused.

r[syntax.nesting-limit]
Primaries, class declarations, chains of setter keywords and pattern
literals nest at most 128 deep. A source nesting one of them deeper is
refused, `expression nested too deeply`, `class nesting too deep` or
`pattern nested too deeply`. A chain of messages, `a b c + d …`, is not a
nesting in this sense: every length of it is accepted.

## Modules

r[syntax.compilation-unit]
A module is one compilation unit: an optional language id, an optional
class category, and one class declaration, the module's class. Nothing
may follow it but whitespace and comments:
`unexpected tokens after the top-level class`.

```text
compilationUnit → [languageId] [classCategory] classDeclaration
languageId → name
classCategory → string
```

r[syntax.language-id]
The language id and the class category are both optional, and neither
has a meaning: a module with them and the same module without them are
the same program. Bracha §8.1 requires the language id; see the appendix.
Because `class` and the access modifiers are names, a leading `class`,
`public`, `protected` or `private` is the language id exactly when a class
category or a class declaration follows it, and begins the class
declaration otherwise: `class class Root = () ()` has the language id
`class` and declares `Root`, `class class = () ()` declares the class
named `class`.

r[syntax.module-class-access]
The module's class takes no access modifier; only a nested class does:
`a module's class takes no access modifier; only a nested class does (§8.1)`.

## Class declarations

r[syntax.class-declaration]
A class declaration is `class`, the class's name, optional type
parameters, the primary factory's message pattern -- or none --, `=`, and
then either a class body, or a superclass clause followed by a class body
or by mixin applications. The alternative with a message pattern is tried
first, so `class Pair = other = Object new () ()` has the binary factory
`=`. A declaration without a message pattern has the factory `new`.

```text
classDeclaration → "class" name [typeParameters] [messagePattern] "="
                   (classBody | inheritanceClause (classBody | mixinApplications))
```

r[syntax.inheritance-clause]
A superclass clause, Bracha's inheritance clause, is an optional prefix --
`outer` and a class name, `self` or `super` -- an identifier naming the
superclass, and at most one message to it: a unary selector, a binary
message or a keyword message, whose arguments are binary or unary
expressions. A second message is refused, `a superclass clause sends at
most one message to the class it names (§6.1)`: `Object new foo`.

```text
inheritanceClause → [inheritancePrefix] name [typeArguments] [message]
inheritancePrefix → "outer" name | "self" | "super"
message → unaryMessage | binaryMessage | keywordMessage
```

r[syntax.mixin-application]
After an explicit superclass clause, one or more mixin applications may
follow, each `<:` and an inheritance clause, ended by a class body or, for
a declaration that adds no members of its own, by `.`:
`class C = S new <: M1 new <: M2 new ( … ) ( … )`, or
`class C = S new <: M new.` The clauses keep their written order.

```text
mixinApplications → ("<:" inheritanceClause)+ ("." | classBody)
```

r[syntax.class-body]
A class body is a header in parentheses, the instance side in
parentheses, and optionally `:` and the class side in parentheses. The
instance side is required, however empty: `class C = () ()`. A header
without the instance side after it is refused, ``expected the instance side's `( ... )` after the class
header (§6.3)``.

```text
classBody → "(" classHeader ")" "(" instanceSide ")" [":" "(" classSide ")"]
```

r[syntax.class-header]
A class header is an optional slot section, sequential or simultaneous,
and then the initializer expressions, separated by single dots, a dot
after the last one allowed. Bracha's `classComment` is a comment and so,
like every comment, no production of the grammar.

```text
classHeader → [slotSection] [expression ("." expression)* ["."]]
```

r[syntax.instance-side]
The instance side is any number of members, each a nested class, a lazy
slot or a method, tried in that order, with nothing between them: a dot
between two members is refused, ``no `.` stands between a class's
members (§6.3)``.

```text
instanceSide → (nestedClass | lazySlot | method)*
nestedClass → [accessModifier] classDeclaration
```

r[syntax.class-side]
The class side is any number of methods, with nothing between them: a dot
between two is refused, ``no `.` stands between a class side's methods
(§6.3)``. A class side declares no slots, lazy slots or nested classes.

```text
classSide → method*
```

r[syntax.access-modifier]
An access modifier is one of `public`, `protected` and `private`. It may
precede a nested class, a slot, a lazy slot and a method.

## Slots

r[syntax.slot-section]
A slot section is a sequential one, between `|` and `|`, or a
simultaneous one, between `||` and `||`. The simultaneous form is tried
first, and an empty sequential section `||` is read when no simultaneous
one matches, so `[:x | || x + 1]` has an empty temporary section (Bracha
§6.3.2 writes the sequential form first, which over characters would make
the simultaneous one unreachable; Bracha's canonical grammar tries the
simultaneous form first, and so does this one).

```text
slotSection → "||" slotDefinition* "||" | "|" slotDefinition* "|"
```

r[syntax.slot-definition]
A slot definition is an optional access modifier, the slot's name, and
then either `=` and an expression -- an immutable slot --, `::=` and an
expression -- a mutable one --, or nothing -- an uninitialized mutable
slot. The dot after an initializer is required, ``expected `.` after a
slot's initializer (§6.3.2)``; after an uninitialized slot it is optional.

```text
slotDefinition → [accessModifier] slotName [("=" | "::=") expression "." | "."]
slotName → name [typeAnnotation]
```

r[syntax.lazy-slot]
A lazy slot is `lazy` and an access modifier in either order, or `lazy`
alone, then a slot's name, `=` or `::=`, an expression and a required dot.
It is a member of the instance side, not of the header. A member is a lazy
slot only if all of that matches: `lazy lazy = 1.` is a lazy slot named
`lazy`, `lazy public public = 1.` a public one named `public`, while
`lazy = ( ^1 )` and `private lazy = ( ^1 )` are methods named `lazy`, and
`lazy public = 1.` is refused.

```text
lazySlot → ("lazy" [accessModifier] | accessModifier "lazy") slotName ("=" | "::=") expression "."
```

## Methods

r[syntax.method]
A method is an optional access modifier, a message pattern, `=`, and the
method's code body in parentheses. A method whose body is not closed is
refused, ``expected `)` to end the method body``.

```text
method → [accessModifier] messagePattern "=" "(" codeBody ")"
```

r[syntax.message-pattern]
A message pattern is a unary selector; or a binary selector and one
parameter -- any binary selector, `||`, `=`, `<` and `->` included; or one or more keywords, each with a parameter. A setter keyword
never begins a message pattern. A parameter is an identifier with an
optional type annotation.

```text
messagePattern → name [typeParameters] [returnType]
               | binaryOperator parameter [returnType]
               | (keyword parameter)+ [returnType]
parameter → name [typeAnnotation]
```

## Type annotations

r[syntax.type-annotation]
A type annotation is accepted where Bracha's typed Newspeak writes one and
has no meaning: a program with its annotations and the same program
without them are the same program. The forms are `<Type>` after a
parameter or a slot's name, `^<Type>` after a message pattern, type
parameters `[T]` after a class's name or a selector, type arguments
`[E]` after a superclass or a mixin clause's receiver and at a value
(`Box[Integer] new`, `a foo[E]`, `(Box)[Integer]`), and a `where { … }`
constraint clause. Inside a `<…>` annotation and a `where { … }` clause,
`<:` is the bound operator. The productions `typeAnnotation` (`<…>`),
`returnType` (`^<…>`), `typeParameters` and `typeArguments` (`[…]`) and
`whereClause` (`where { … }`) each take the tokens up to the bracket that
closes their own, nested brackets of the same kind included. An annotation is kept as the tokens written; an
unclosed one is refused, ``unterminated `<...>` type annotation``,
``unterminated `[...]` type arguments``, ``unterminated `where { ... }`
clause``.

r[syntax.type-parameters]
A class's type parameters are written either as Bracha §6 writes them,
`class Map <[K, V]> new: capacity <Integer>`, or as Bracha's canonical
grammar does, `class List[E]`. Both are type annotations
([`syntax.type-annotation`](#r-syntax.type-annotation)).

## Metadata

r[syntax.metadata]
A metadata comment ([`lex.comment.metadata`](lexical-structure.md#r-lex.comment.metadata))
belongs to what it stands next to, nothing but whitespace and comments
between them (Bracha §4.3): one immediately before an expression belongs
to that expression, one immediately before a slot or a parameter to that
slot or parameter, one immediately after a method's or a factory's message
pattern to that method or factory, and one immediately after a class's
header to that class. One immediately before a module's class declaration
belongs to the module's class. Several metadata comments may belong to
one target. A metadata comment that stands next to none of these belongs
to nothing.

## Code bodies and statements

r[syntax.code-body]
A code body -- a method's, a block's -- is an optional temporary section,
a slot section with the full form of slot definitions
([`syntax.slot-definition`](#r-syntax.slot-definition)), and then its
statements: `| x = 1. y |` declares an initialized immutable temporary
`x` and an uninitialized mutable one `y`.

```text
codeBody → [slotSection] statements
```

r[syntax.statements]
Statements are expressions separated by single dots, a dot after the
last one allowed, and may end with a return statement, `^` and an
expression with an optional dot after it. Nothing follows a return:
``expected the end of the body after a return (§7.2)``. A dot that ends
no statement is refused, ``a `.` ends a statement, and none is here``:
`( . self foo )`, `( self foo.. self bar )`.

```text
statements → "^" expression ["."]
           | expression ["." statements]
           | empty
```

## Expressions

r[syntax.expression]
An expression is any number of setter keywords followed by a send
expression: `a:: b:: 3`. A setter keyword stands only at the head of an
expression -- ``a setter send `name:: value` stands only at the start of
an expression (§5.4.3)`` -- so `self foo:: 3` and `self foo; bar:: 3` are
refused, and a setter keyword never begins a message pattern. Bracha
§5.4.3 writes one optional setter keyword, yet gives as its rationale "to
enable chaining of setter sends, e.g., `w::x::y`"; the chain is accepted.

```text
expression → setterKeyword* sendExpression
sendExpression → keywordMessage | primary [messages]
```

r[syntax.implicit-keyword-send]
A keyword message with no receiver before it is sent to the implicit
receiver: `at: 1 put: 2` alone is a send expression. It cannot be
cascaded: ``a keyword send to the implicit receiver cannot be cascaded
(§5.4.4)``.

r[syntax.precedence]
After a primary come unary messages, then binary messages, then at most
one keyword message: unary binds tighter than binary and binary tighter
than keyword, and binary messages associate to the left. A binary
message's argument is a primary with its unary messages; each argument of
a keyword message is a primary with its unary and binary messages.

```text
messages → unaryMessage+ binaryMessage* [keywordMessage] [cascade]
         | binaryMessage+ [keywordMessage] [cascade]
         | keywordMessage [cascade]
unaryMessage → ["<-:"] unarySelector [typeArguments]
binaryMessage → ["<-:"] binaryOperator primary unaryMessage*
keywordMessage → ["<-:"] (keyword primary unaryMessage* binaryMessage*)+
```

r[syntax.binary-operator]
Wherever a binary message can stand, its selector is a binary selector or
one of the punctuation tokens `=`, `<`, `>`, `|` and `||`: `a = b`,
`a < b`, `a | b`. A selector with no operand after it is not taken as a
message, so the `|` that closes a slot section stays one.

```text
binaryOperator → binarySelector | "=" | "<" | ">" | "|" | "||"
```

r[syntax.cascade]
A cascade follows a primary and at least one message sent to it; each
`;` and a unary, binary or keyword message sends that message to the same
receiver as the message before the first `;`. A cascade after a primary
alone is refused, ``a cascade follows a message sent to its receiver
(§5.4.4)``: `self; foo`, `(self foo); bar`. A `;` with no message after it
is refused, ``expected a message after `;` ``.

```text
cascade → (";" (keywordMessage | binaryMessage | unaryMessage))+
```

r[syntax.eventual-send]
An asynchronous send writes `<-:` before its message, wherever a unary,
binary or keyword message or a cascade's message stands: `self <-: foo`,
`self <-: + 1`, `self <-: at: 1`, `self foo; <-: bar`: the optional `<-:` of the
productions of [`syntax.precedence`](#r-syntax.precedence). A `<-:` with
no message after it is refused, ``expected a message after `<-:` (§5.6)``. What an asynchronous
send does is the semantics' to say ([Semantics](semantics.md)).

## Primaries

r[syntax.primary]
A primary is a name, a literal, a block, a parenthesized expression,
`self`, `super` with its message, or an outer send. A parenthesized
expression is `(`, an expression and `)`; parentheses change grouping and
nothing else. Every primary may be followed by type arguments,
`Box[Integer] new`, `(Box)[Integer]`, which have no meaning
([`syntax.type-annotation`](#r-syntax.type-annotation)).

```text
primary → (name
          | literal
          | block
          | "(" expression ")"
          | "self"
          | "super" message
          | outerSend) [typeArguments]
literal → number | string | symbol | tuple | pattern | "true" | "false" | "nil"
```

r[syntax.self]
`self` is a primary, the receiver of the current method. `true`, `false`
and `nil` are literals ([`syntax.literal`](#r-syntax.literal)).

r[syntax.super]
`super` is always followed by a message, unary, binary or keyword: a
`super` without one is refused, ``` `super` is followed by a message
(§5.10) ```.

r[syntax.outer.send]
An outer send is `outer`, the name of a lexically enclosing class, and a
message: `outer Bank balance`. `outer` not followed by a name is refused,
``` `outer` is followed by the name of an enclosing class (§5.9) ```. A
name that names no enclosing class is an error of the resolver's
([Semantics](semantics.md)).

r[syntax.outer.alone]
`outer` and the name of an enclosing class with no message after them is
a primary of its own, the enclosing object an outer send to that class
would send its message to: `outer Bank` alone answers the enclosing
`Bank`. Bracha §5.9 defines `outer N` only as a send's receiver; see the
appendix.

```text
outerSend → "outer" name [message]
```

## Literals

r[syntax.literal]
A literal is a number, a string, a symbol, a tuple, a pattern, or one of
`true`, `false` and `nil`. Bracha's character constant is a one-character
string ([`lex.string.one-character`](lexical-structure.md#r-lex.string.one-character)).

r[syntax.literal.number]
A number token stands as a literal wherever a primary stands. Where a
binary message stands, a number token that begins with `-` is read as the
binary operator `-` and the number without its sign
([`lex.number.sign`](lexical-structure.md#r-lex.number.sign)): `3 -4` is
`3 - 4`, while `{3. -4}` and `foo: -4` hold negative numbers. A radix
number's radix is between 2 and 36, and each of its digits is less than
its radix: ``the radix of `37r1` is not between 2 and 36 (§5.1.1)``,
``` `2` is not a digit of radix 2 in `2r12` (§5.1.1) ```.

r[syntax.tuple]
A tuple is `{`, expressions separated by single dots, a dot after the
last one allowed, and `}`: `{}`, `{1. 2}`, `{1. 2.}`. Two elements without
a dot between them are refused, ``expected `.` between a tuple's
elements``.

```text
tuple → "{" [expression ("." expression)* ["."]] "}"
```

r[syntax.block]
A block is `[`, optional parameters -- each `:` and a name with an
optional type annotation, the last followed by `|` -- a code body, and
`]`: `[:each | each + 1]`, `[:x :y | | sum | sum:: x + y. sum]`, `[]`.

```text
block → "[" [(":" parameter)+ "|"] codeBody "]"
```

r[syntax.pattern]
A pattern literal is `<`, a pattern and `>`: `_`, a literal -- a number,
a symbol, a string or a tuple -- or one or more keywords each with an
optional value, `_`, a literal, `?` and a name, or a nested pattern; a
keyword's value may be left out before `>` and before the next keyword,
`<x:>`, `<x: y: 1>`.
`<x: <y: 1>>` closes two patterns with the one token `>>`, which the
parser reads as two `>`. A pattern none of these forms is refused,
``expected `_`, a literal or a keyword in a pattern (§5.1.9)``. What a
pattern literal evaluates to is the semantics' to say
([Semantics](semantics.md)).

```text
pattern → "<" ("_" | patternLiteral | (keyword [patternValue])+) ">"
patternLiteral → number | symbol | string | tuple
patternValue → "_" | patternLiteral | "?" name | pattern
```

## Gaps

r[syntax.gap.object-literal]
Bracha §5.1.10 defines object literals, `objectLiteral → [identifier
[keywordMsg]] classBody`, an anonymous class declaration that evaluates
to a new object, and marks them *Unimplemented*. ns-next does not parse
them: the production is reachable from no other, so Bracha does not say
where one may stand, and it is ambiguous with a parenthesized expression
and a keyword send. This is a gap.

r[syntax.gap.chain]
Bracha §5.4.4 describes chains, `e :| μ`, equivalent to `(e) μ`, marks
them *Unimplemented*, gives them no grammar, and leaves open whether they
stay in the language. ns-next does not parse them, and `:|` is no token.
This is a gap.

r[syntax.gap.class-expression]
Bracha §6 intends a class declaration to be permitted as an ordinary
expression inside a method or a closure, and marks it *Unimplemented*.
ns-next does not parse one there. This is a gap.
