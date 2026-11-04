# MCPL language specification v0.1

This is the target language contract, not a claim that the current proxy stub
implements it. The design authority is the MCPL language-design interview,
rounds 1–8 (project ticket #1). Later decisions supersede earlier ones: prefix
unary minus is supported, and `help` and `show` are MCP tools, not builtins.
Round 8 adds raw strings and `;` sequencing with locals, permits lexical
shadowing, and guarantees termination through fuel rather than by construction.

Details not settled by that interview are explicitly labelled
**(implementation-defined; proposed: …)**. These proposals make examples and
interfaces concrete without disguising them as interview decisions. Type
notation below describes types; it is not additional executable MCPL syntax.

## 1. Overview and goals

MCPL (Model Context Protocol Language) is a small, statically typed scripting
language for composing MCP tool calls and manipulating their results without
sending intermediate data through an LLM. Types come from downstream discovery
and JSON Schema. Discovery and progressive disclosure replace wholesale schema
and result dumps. MCPL has no loops, recursive declarations, filesystem, shell,
network primitives, or I/O other than downstream tool calls. Iteration is through
bounded collection builtins. Termination is guaranteed by the fuel budget, not
by construction: evaluation finishes or fails, subject to unbounded waiting for
external tools. Calling `any` values, including self-application, is legal.
It is not JavaScript, TypeScript, an OS sandbox, or an MCP resources/prompts proxy.

The runtime is an MCP server exposing exactly `run`, `help`, and `show`. A registry
retains downstream server names from configuration, tool names, descriptions,
and raw input/output schemas. A server is a value whose fields are tools. The
registry can be shared, but each MCP connection has its own `SessionState`:
bindings, reserved names, automatic-name counter, and memory accounting. Names
are case-sensitive and tools are not flattened into a global namespace.

Each top-level semicolon-terminated statement creates one immutable session binding.
Explicit assignments choose its name; expression statements receive `resN`.
These names are the handles: subsequent programs can refer to them directly.
`run` returns small typed previews, `help` discovers signatures and documentation,
and `show` discloses an expression without creating another binding. Bindings
survive failures in later statements and later runs, until eviction or disconnect.

## 2. Agent-facing quick reference

- Write `name = expression;` or `expression;`. Always terminate statements with `;`.
- Use JSON values, optionally bare object keys and trailing commas. Strings use
  double quotes and JSON escapes; raw strings `r"..."` take no escapes. Use raw
  strings for regex patterns. There is no `let` or statement block.
- Call `server.tool({key: value})`; use `help` first to discover the signature.
- Read `x.field`, `x["field"]`, or `xs[0]`. Dotted/hyphenated names are preserved.
- Put whitespace on both sides of every **binary** operator. `a-b` is a name;
  `a - b` is subtraction. Unary `-x` and `!flag` need no separating whitespace.
- Use `c ? yes : no` and `map(xs, (x) => expression)`. No loops or method-style
  collection operations. Conditions are booleans, not truthy values.
- Every top-level statement has an output binding, including tool calls and
  assignments. Session names can never be reassigned, even after eviction.
- Inside parentheses, array elements, object values, or call arguments, sequence
  with `;`: `(x = f(); g(x))`. Only the final expression supplies the value;
  locals never create session bindings or output. Evaluation is left-to-right.
- Lambda parameters and sequence locals may shadow any visible name. A name
  may be defined only once per scope; the innermost scope wins.
- A syntax/type error executes nothing. A runtime failure stops at that statement,
  keeping earlier bindings. Tool side effects are not rolled back.
- Use the separate MCP tools `help` for documentation and `show` for more output.
  They cannot be called inside MCPL expressions.

### Normative `RUN_DESCRIPTION`

The following is the **verbatim approved text** for `RUN_DESCRIPTION`:

```text
Run an MCPL program. MCPL is a small statically typed scripting language for calling and combining the tools of the connected MCP servers without their results passing through you.

A program is a sequence of statements, each ending in `;`: either `name = expression;` or `expression;`. Every statement creates a binding that persists for this session; unnamed ones are called res1, res2, …. The result lists each new binding as `name: type (size) = truncated preview`. Later programs refer to bindings by name to drill in (`res1[0].title;`). A name can never be reassigned.

Values are JSON (keys may be unquoted, trailing commas allowed); raw strings `r"..."` take no escapes — use them for regexes. Call a tool as `server.tool({...args})`; tools without an output schema return `Content[]`, and `text(x)` gives their text; use `parse_json(text(x))` when that text is JSON. Fields: `x.key` or `x["key"]`; items: `x[i]`. Lambdas: `(x) => expr`. Inside parentheses or other brackets, `;` sequences expressions and `name = expr;` makes a local: `(x = f(); g(x))`. Operators `+ - * / == != < <= > >= && || !` must have spaces around them (names may contain `-`); `c ? a : b`; `+` also joins strings. Conditions must be boolean; there are no implicit conversions and no loops — iterate with built-ins like map, filter, reduce. Any error (bad index, failed tool call, exceeded limit) stops the program; earlier bindings are kept.

Use `help` to list servers, tools, signatures and built-ins, and `show` to read a value in full.
```

The description is deliberately short, not the grammar. Its spaces rule applies
to binary operators; unary `!` and `-` follow section 3. `{...args}` in that prose
is a schematic placeholder for object fields, **not spread syntax**. “Other
brackets” permits sequences in array elements, object values, and call arguments,
not directly in index expressions (section 4). The description's statement and
persistent-name rules concern session bindings, not lexical locals. JSON describes
the data model, not the first-class function/server/tool values of section 7.

## 3. Lexical structure

### Symbols and keys

A symbol has the shape `part ('.' part)*`, where `part = [A-Za-z0-9_-]+`.
The first part cannot begin with a digit or `-`; it starts with `[A-Za-z_]`.
Later parts can start with either. There are no leading, trailing, or consecutive
periods. The full symbol is 1–128 ASCII characters, matching the adopted MCP
name-length limit. `true`, `false`, and `null` are reserved literals and cannot
be root parts, but are legal member segments. Thus `s.2fa-check({})` and `x.true`
are legal, while `2fa-check = s.2fa-check;` is a syntax error with
`hint: choose a name starting with a letter or '_'`.

```text
Valid symbols:   x  _tmp  browser-navigate  a.b  a.2b  a.-b
Not symbols:     2fa-check  -x  .a  a.  a..b  true
```

Every tool name remains accessible through an exact string index:
`server["2fa-check"]`, `server["tool..name"]`, `server["true"]`. Leading-digit,
leading-dash, and reserved member names also work after a valid root:
`server.2fa-check`, `server.-tool`, `server.true`. Only names violating the
member spelling rules, such as `tool..name`, require index syntax.
No tool is renamed. A server's configured name is the global root name.
Configuration with an unspellable or colliding global server name needs a policy
**(implementation-defined; proposed: reject it at language-environment setup with
an actionable configuration error; require a valid, nonreserved symbol not equal
to a builtin)**. This does not restrict downstream tool names.

Bare object keys use symbol spelling and denote the entire spelling literally:
`{a.b: 1}` has one key `"a.b"`, not a nested object. Quoted keys can be any string.
Reserved words as bare keys are unspecified **(implementation-defined; proposed:
require quoted keys for `"true"`, `"false"`, and `"null"`)**. Object keys do not
perform name lookup. Member segments after `.` can use the full `part` alphabet;
for example `x.true` reads a field and `x.2b` is valid member spelling.

### Numbers

Number tokens follow JSON's grammar:

```ebnf
number = ["-"], ("0" | nonzero-digit, {digit}),
         [".", digit, {digit}], [("e" | "E"), ["+" | "-"], digit, {digit}] ;
```

There are no leading `+`, leading zeros, numeric separators, hexadecimal numbers,
`NaN`, or `Infinity` literals. `.5` and `1.` are invalid; `0.5`, `-5`, `1e-3`, and
`2E+4` are valid. The parser may represent a negative JSON number as unary minus
applied to a positive number without changing its value. Non-finite f64 results,
including an overflowing literal, fail rather than becoming values.
Overflowing-literal diagnosis is unspecified **(implementation-defined; proposed:
a type error before evaluation)**.

### Strings

Normal strings are double-quoted JSON strings. Escapes are exactly:

```text
\"  \\  \/  \b  \f  \n  \r  \t  \uXXXX
```

Unescaped control characters are invalid. No single quotes, template strings,
interpolation, `\xNN`, `\0`, `\v`, or braced Unicode escapes are accepted.
Values are Unicode strings; paired JSON UTF-16 surrogate escapes decode to one
Unicode scalar. Unpaired surrogate escapes cannot represent a scalar
**(implementation-defined; proposed: syntax error, with a hint to supply a pair)**.
JSON interchange with otherwise valid lone-surrogate strings is therefore an
explicit compatibility boundary.

Raw strings have the form `r"..."`, perform **no escape processing**, and end at
the next `"`. They may span lines but cannot contain a quote; use a normal
string when a quote is needed. Both forms have type `string`. An `r` immediately
followed by `"` begins a raw string, never a symbol.

**Use raw strings for regex patterns:** `r"\bRust\b"` preserves word-boundary
escapes and `r"\d+"` preserves the digit-class escape. In a normal string,
`"\bRust\b"` contains backspaces, not regex word boundaries, and `"\d"` is a
syntax error because `\d` is not a JSON escape.

### Whitespace, comments, and operators

JSON whitespace is space, tab, CR, and LF. `//` begins a line comment outside a
string; a comment ends at CR, LF, or end of input. Block comments do not exist.

Binary operators require whitespace on **both** sides:

```mcpl
a-b;          // One symbol lookup.
a - b;        // Subtraction.
a == b;       // Equality.
```

`a==b` is a syntax error, with `hint: add whitespace on both sides of '=='`.
Tabs and newlines count. Comment interaction is unspecified
**(implementation-defined; proposed: comments can occur in separating trivia,
but each binary operator must have an actual whitespace character immediately
before and after it; a comment alone is not a substitute)**.

Whitespace is optional around `=`, `=>`, `?`, `:`, `,`, delimiters, and `;`.
Prefix unary `!` and `-` do not require whitespace: `!ok`, `-x`, and `-(a + b)`.
Maximal symbol matching makes `a-b` one name; `a -b` is neither subtraction nor a
valid adjacent expression. `-a-b` negates the symbol `a-b`.

## 4. Grammar

The following PEG uses ordered choice `/`, repetition `*`/`+`, optional `?`,
positive lookahead `&`, negative lookahead `!`, and end-of-input `EOF`.
Quoted punctuation is literal; character classes are ASCII unless stated.
Productions do **not** consume implicit trailing whitespace.

```peg
Program       <- S (Statement S)* EOF
Statement     <- Assignment / Expression S ';'
Assignment    <- &(Symbol S Assign) Symbol S Assign S Expression S ';'
Assign        <- '=' ![=>]

Sequence      <- (SeqItem S ';' S)* Expression
SeqItem       <- &(Symbol S Assign) Symbol S Assign S Expression / Expression
Expression    <- Lambda / Conditional
Lambda        <- '(' S Parameters? S ')' S '=>' S Expression
Parameters    <- Symbol (S ',' S Symbol)*
Conditional   <- Or (S '?' S Expression S ':' S Expression)?
Or            <- And (BOr And)*
And           <- Equality (BAnd Equality)*
Equality      <- Comparison (BEq Comparison)*
Comparison    <- Additive (BCmp Additive)*
Additive      <- Product (BAdd Product)*
Product       <- Unary (BMul Unary)*
Unary         <- ('!' / '-') S Unary / Postfix
Postfix       <- Primary (Call / Index / Member)*
Call          <- S '(' S Arguments? S ')'
Arguments     <- Sequence (S ',' S Sequence)*
Index         <- S '[' S Expression S ']'
Member        <- '.' Part ('.' Part)*
Primary       <- Null / Boolean / UnsignedNumber / RawString / String / Object / Array
               / Symbol / '(' S Sequence S ')'

Object        <- '{' S (Pair (S ',' S Pair)* (S ',')?)? S '}'
Pair          <- (String / Symbol) S ':' S Sequence
Array         <- '[' S (Sequence (S ',' S Sequence)* (S ',')?)? S ']'
Null          <- 'null' !NameChar
Boolean       <- ('true' / 'false') !NameChar
Symbol        <- !Reserved FirstPart ('.' Part)*
Reserved      <- ('true' / 'false' / 'null') ![A-Za-z0-9_-]
FirstPart     <- [A-Za-z_] [A-Za-z0-9_-]*
Part          <- [A-Za-z0-9_-]+
NameChar      <- [A-Za-z0-9_.-]
UnsignedNumber <- ('0' / [1-9] [0-9]*) ('.' [0-9]+)?
                  ([eE] [+-]? [0-9]+)? ![A-Za-z0-9_-]
RawString     <- 'r' '"' (!'"' .)* '"'
String        <- '"' (Escape / StringScalar)* '"'
Escape        <- '\\' (["\\/bfnrt] / 'u' Hex Hex Hex Hex)
Hex           <- [0-9A-Fa-f]
StringScalar  <- /* one Unicode scalar except U+0000..U+001F, '"', '\\' */

BOr           <- Before '||' After
BAnd          <- Before '&&' After
BEq           <- Before ('==' / '!=') After
BCmp          <- Before ('<=' / '>=' / '<' / '>') After
BAdd          <- Before ('+' / '-') After
BMul          <- Before ('*' / '/') After
Before        <- S &PreviousWhitespace
After         <- &W S
PreviousWhitespace <- /* predicate: preceding source character is W */
S             <- (W / Comment)*
W             <- [ \t\r\n]
Comment       <- '//' (!('\r' / '\n') .)*
EOF           <- !.
```

The source predicates in `Before`/`After` preserve the surrounding-whitespace rule even when trivia is
consumed speculatively. Numeric tokenization must reject a malformed JSON number
as a whole rather than accepting a valid prefix. Symbol length and decoded
Unicode validity are lexical checks in addition to these productions.

A `Symbol` primary retains its segments for static resolution, rather than
committing to a single variable token or a sequence of field lookups. Adjacent
member segments are similarly resolved together (section 5). Separating a dot
from its neighboring parts is unspecified **(implementation-defined; proposed:
no whitespace around a member dot; use index syntax if needed)**.

The optional trailing comma applies to arrays and objects. Trailing commas in
calls and parameter lists were not decided **(implementation-defined; proposed:
reject them, as this grammar does)**. Empty programs and zero-parameter lambdas
are likewise **(implementation-defined; proposed: accept both; an empty program
creates no bindings)**. Duplicate parameter names are a type error:
`(x, x) => x` reports `x is already defined in this scope`. Destructuring and
parameter defaults do not exist.

`Sequence` is admitted **only directly inside brackets**: a parenthesized
primary, an array element, an object pair value, or a call argument. Each is
delimited by a comma or closing bracket. Lambda bodies, ternary branches, index
expressions, and top-level statements remain `Expression`; use parentheses to
sequence there. Otherwise `f = (x) => y = x + 1; y * 2;` would be ambiguous
between a lambda-local sequence and top-level statements. Write
`f = (x) => (y = x + 1; y * 2);` instead. A sequence ends in an expression, not
an assignment or trailing semicolon. The program is the degenerate case: every
top-level item ends with `;`, with no final unterminated expression; its grammar
and session-binding semantics are unchanged.

Precedence within `Expression`, from tightest to loosest:

| Level | Forms |
| --- | --- |
| Postfix | `f(args)`, `x[i]`, `x.key` |
| Unary | `!`, prefix `-` |
| Multiplicative | `*`, `/` |
| Additive | `+`, binary `-` |
| Relational | `<`, `<=`, `>`, `>=` |
| Equality | `==`, `!=` |
| Logical AND | `&&` |
| Logical OR | `||` |
| Conditional | `condition ? yes : no` |
| Lambda | `(parameters) => expression` |

Associativity is **(implementation-defined; proposed: binary levels associate
left; unary operators and conditional/lambda bodies associate right)**. Thus
`8 / 4 / 2` is `(8 / 4) / 2`; `a ? b : c ? d : e` nests in the last branch.
A lambda used as an arithmetic operand needs grouping, though ordinary call
arguments accept lambdas directly. `(x) => x + 1` has one expression body.
`(x) => {value: x}` returns an object; braces never introduce a block.

There are **no** declarations with `let`, blocks, loops, `%`, `??`, template
strings, single-quoted strings, spread, computed object keys, named arguments,
method-style collection operations, general assignment expressions, or user type
annotations. Local definitions are permitted only as non-final sequence items.
There is no general recursion or forward reference facility.

## 5. Name resolution

Resolution is static, using the environment's variables, server names, builtins,
and derived types. Do not discover a tool by trying a shorter runtime value first.

1. Consult lexical scopes **innermost-first**, then session/server/builtin roots.
   In the first scope with a matching root, choose the **longest leading sequence
   of segments** that exactly names a binding. That is the root. An inner lexical
   match takes precedence over any outer match, including a longer name.
2. At each subsequent static record/server step, choose the **longest join** of
   the remaining segments that is a known member name. Consume it and continue.
3. On `any`, each remaining segment is exactly one key. Do not guess dotted keys
   from the runtime object's contents.
4. An explicit string index is one exact key and bypasses dotted joining.
   `server["tool.name"]` and `server["2fa-check"]` always select those names.
5. Missing roots or statically absent members are type errors. A missing key on
   `any` or an index-signature record is checked at runtime.

The longest match is greedy, not a search with backtracking. If a longer match
exists but its type does not support the remaining suffix, report that error;
do not silently fall back to a shorter root. These rules make multiple possible
splits deterministic, not ambiguous.

```mcpl
a = {b: {c: 1}};
a.b = {c: 2};
a.b.c;                 // Root a.b, then member c: 2.
a["b"].c;              // Root a, then exact key b, then c: 1.
```

`a.b = 1;` defines a global named literally `a.b`; it never mutates `a`.
For a server `s` with tools `x`, `x.y`, and a no-fields tool `2fa-check`:

```mcpl
s.x;                   // The x tool, a function value.
s.x.y;                 // The x.y tool, not a member of the x function.
s["x.y"];              // The same x.y tool.
s["2fa-check"];        // Also reachable even though not a symbol root.
s.2fa-check({});       // A member segment may start with a digit.
```

For a record whose keys are `"x"` and `"x.y"`, `r.x.y` selects `"x.y"`;
`r["x"].y` explicitly traverses `"x"`. For `r: any`, `r.x.y` always traverses
`"x"` then `"y"`; `r["x.y"]` selects the dotted key.

Union member lookup is unspecified **(implementation-defined; proposed: a joined
member must be known in every possible record variant to be directly readable;
otherwise narrow first. A declared optional member is still known)**. A union
containing `any` follows gradual rules. String-literal indexing of a server
preserves the exact tool type; dynamic string indexing is
**(implementation-defined; proposed: yield the union of its tool types, requiring
an argument valid for every possible callable signature)**.

`xs.map(f)` parses as member access followed by a call, but for `xs: string[]`
is a **type error**: `map is not a property of string[]; hint: call map(xs, f)`.
Function-valued record fields remain callable: `record.f(x)` is an ordinary call.

There is no remaining syntax-level ambiguity after longest-match resolution.
Conflicting roots, such as a server and builtin with the same name, must not be
silently selected. Exact diagnostic templates are **(implementation-defined;
proposed)**:

```text
type error: ambiguous name 'map' (server and builtin)
  hint: rename the server in configuration

type error: unknown name 'playwrigt'
  hint: did you mean 'playwright'?

type error: property 'missing' does not exist on {x: number}
```

Suggestions appear only when a useful nearby name exists. Tie-breaking for
suggestions is **(implementation-defined; proposed: edit distance, then lexical
order)**. Full source layout is specified in section 13.

## 6. Bindings and sessions

### Creation and immutability

Every completed top-level statement commits exactly one binding:

```mcpl
answer = 42;     // Explicit name answer.
answer + 1;      // Automatic name res1 in a fresh session.
```

`resN` uses an incrementing per-session counter starting at 1, not a per-run
counter. Explicit assignments do not consume automatic numbers. Collision and
failure behavior is **(implementation-defined; proposed: during preflight assign
auto-names in statement order, skipping already reserved names; reserve a name
and advance the persistent counter only when its statement commits. Reject a
collision with another explicit declaration in the same program)**. An explicit
`res100 = 1;` is legal if the name is unused; later automatic allocation skips it.

Preflight sees earlier committed bindings, earlier declarations within this
program, all ever-used names including evicted ones, builtins, and server roots.
Reassigning any reserved name is a type error **before any statement executes**.
Even `x = 1; x = 2;` executes neither statement. Top-level assignments cannot
shadow builtins or servers. A right-hand side cannot refer to its own new name or a
later declaration: this prevents recursive declarations and forward references.

### Sequences and lexical locals

Sequence items evaluate left-to-right. Non-final expression items are evaluated
for effect and their values discarded. `name = expr;` inside a sequence binds a
local after evaluating `expr`, visible from that point to the end of the
enclosing sequence and in nested scopes. A sequence's value and type are those
of its final expression. Each array element, object value, and call argument
has its own sequence scope; locals do not leak into adjacent elements, fields,
or arguments. Locals are never session bindings, never allocate `resN`, and
never appear in `run` output.

Lambda parameters are also lexical, not session bindings. Parameters and sequence
locals may shadow **anything visible**: session bindings, builtins, server names,
and enclosing lexical names. The innermost scope wins; `(text) => upper(text)`
is legal, and `text` in its body means the parameter, not the builtin. A name
may be defined only once per scope: `(x, x) => x` and `(x = 1; x = 2; x)` are
type errors, `x is already defined in this scope`. A local initializer cannot
refer to its own new binding; an outer binding of the same name remains visible
until the local is defined. These rules do not relax top-level immutability.

For example, in a fresh session:

```mcpl
{xyz: local1 = 123; {abc: local1}};
```

```text
res1: {xyz: {abc: number}} (1 key) = {xyz: {abc: 123}}
```

Only `res1` persists; `local1` is no longer in scope.

### Three runs in one session

Run 1:

```mcpl
{x: 123};
```

```text
res1: {x: number} (1 key) = {x: 123}
```

Run 2:

```mcpl
{y: res1};
```

```text
res2: {y: {x: number}} (1 key) = {y: {x: 123}}
```

Run 3:

```mcpl
res2 = 32;
```

```text
type error: res2 is already defined
 --> 1:1
  |
1 | res2 = 32;
  | ^^^^
  hint: choose a new name; session bindings cannot be reassigned
```

Run 3 creates nothing and does not change either previous value.

### Lifetime, memory, and eviction

Bindings last for the MCP connection, not the lifetime of a particular agent
loop sharing that connection. Disconnect drops the entire session. Session
memory is capped at 256 MB. On overflow evict least-recently-used bindings.
A **use** is a global variable lookup of that name during evaluation, including
lookups in a `show` expression. `help`, static checking, output previewing, and
reading a lambda parameter or sequence local do not use a global binding.

Evicted names remain reserved. Their types must remain available for preflight,
so using an evicted name is a runtime error, not an unknown-name type error:

```text
runtime error: `res3` was evicted (session memory limit); recompute it
```

Implementation accounting is **(implementation-defined; proposed: deterministic
logical retained-value bytes, including closures and their captures; shared
allocations counted once, names/type tombstones included, and equal LRU ages
broken by creation order. Newly committed bindings begin most-recently-used)**.
Values in active evaluation are not invalidated by eviction. Evicting a name
may free no memory if another binding or closure retains the same value.

Closure capture versus later eviction is **(implementation-defined; proposed:
lexically capture immutable values when constructing a closure. Resolving a
global capture counts as a lookup then; later reads of that captured local do
not refresh the original global name. Eviction removes access through the name,
not data retained by a closure)**.

If eviction cannot make room, including a single retained value larger than the
cap or excessive tombstone metadata, report `limit exceeded` and do not commit
the failing statement **(implementation-defined; proposed)**. Permanent name
reservation must not become an unbounded-memory escape hatch. Active temporary
allocation accounting is **(implementation-defined; proposed: use the same memory
budget for retained values plus evaluation temporaries, evicting eligible global
bindings first)**. Earlier bindings can themselves have been evicted by normal
memory pressure; failure does not restore evicted values.

Concurrent requests against one session need serialization
**(implementation-defined; proposed: serialize evaluation and disclosure requests
per connection to keep name allocation and LRU order deterministic)**.

## 7. Values and types

Types are rendered in TypeScript style everywhere: help, bindings, and errors.
There are no user-written type declarations, casts, generic arguments, or imports.
Generic metavariables such as `T` in builtin documentation describe a family of
signatures, not source syntax.

| Type form | Meaning |
| --- | --- |
| `string` | Unicode scalar-value string |
| `number` | Finite IEEE 754 binary64 (f64) |
| `integer` | Informational schema rendering; checks as `number` |
| `boolean` | `true` or `false` |
| `null` | The null value |
| `any` | Gradual type, with runtime checks at use sites |
| `T[]` | Homogeneous array |
| `[A, B]` | Tuple, with a type for each position |
| `{a: string, b?: number}` | Record with required `a`, optional `b` |
| `{[key: string]: T}` | Record with dynamic string keys |
| `"a"`, `42`, `true` | Literal type |
| `A | B` | Union; a value is a member of either alternative |
| `(x: T) => U` | Function type |
| `{navigate: (args: {url: string}) => Content[]}` | Server type, a record of tools |

Parenthesize unions in arrays: `(string | null)[]`. A record can combine named
fields and a dynamic index signature. Named fields take precedence: the signature
applies only to additional keys, not to the named fields. A dynamic string-key
read has the union of the signature type and all named-field read types (section
9); a missing dynamic key still fails at runtime. Property ordering and union
normalization are **(implementation-defined; proposed: preserve declaration/schema order,
remove duplicate alternatives, and render unsafe bare keys as JSON strings)**.
`integer` remains display metadata, not a second numeric type or an implicit
conversion. Constraints such as integrality are checked against the original
schema at a call boundary.

Ordinary numeric and string literals need a widening policy
**(implementation-defined; proposed: infer `number`/`string` for ordinary binding
listings while retaining literal information for assignability, literal tuple
indexes, and discriminants; schema `enum`/`const` keep explicit literal types)**.
Record discriminants can retain their literal type when checking a union.

Numbers render without `.0` when integral. Precision is f64 precision, not
arbitrary-precision JSON decimal arithmetic. `len`, `substring`, and `index_of`
use Unicode scalar positions, not UTF-8 bytes, UTF-16 code units, or grapheme
clusters. String ordering is **(implementation-defined; proposed: lexicographic
Unicode scalar order, without locale collation or normalization)**. Negative zero
rendering is **(implementation-defined; proposed: render as `0`)**.

Functions, servers, and tools are first-class but **not JSON**. A server's type is
its tool index, not a magical function/record hybrid. A tool value retains its
server identity and schema checks when assigned or passed to a builtin:

```mcpl
nav = playwright.browser_navigate;
nav({url: "https://example.com"});
```

Their binding previews, `show` results, and `to_json` render **only their type**.
`to_json` therefore returns a string containing the type for a non-JSON value;
it does not serialize an executable capability. Equality on such values is a
type error. Containers holding these values are also not JSON
**(implementation-defined; proposed: render the whole container's type for
`to_json`, and reject equality on containers known to contain capabilities)**.
A value reaching equality through `any` gets the corresponding runtime check.

## 8. Type derivation from JSON Schema

### Principles

Derivation targets JSON Schema 2020-12, with best-effort interpretation of
`$schema` for earlier dialects. **Derivation never fails.** Unsupported,
malformed, unresolved, or unrepresentable constructs degrade conservatively to
`any`; they must not make the entire discovery process fail. The raw schema is
retained separately for validation. A derived type is an approximation, not a
replacement for a JSON Schema validator.

Derivation may retain expressible surrounding structure while an unsupported
subschema becomes `any` **(implementation-defined; proposed: preserve known
properties/items wherever sound, but never pretend an unsupported constraint
has been enforced statically)**. For schema constraints that cannot be expressed
in this type system, sound overapproximation is preferred to rejecting discovery.

### Keyword mapping

| Keyword or schema form | Derived type / handling |
| --- | --- |
| `true`, `{}` | `any` |
| `false` | No accepted JSON values; no bottom-type syntax was adopted, so `any` plus call-time rejection (implementation-defined; proposed) |
| Missing `type` and no structural clues | `any` |
| `type: "string"` | `string` |
| `type: "number"` | `number` |
| `type: "integer"` | Checks as `number`, rendered `integer` |
| `type: "boolean"` | `boolean` |
| `type: "null"` | `null` |
| `type: "object"` | Record; without restrictions an open `{[key: string]: any}` |
| `type: "array"` | Array or tuple using the rules below |
| `type: ["string", "null"]` | Union of the listed types |
| `enum` | Union of literal types; composite values conservatively approximated by their record/array shapes (implementation-defined; proposed) |
| `const` | Literal type for scalar values; composite treatment as for `enum` |
| `properties` | Named fields with recursively derived types |
| `required` | Listed named fields required; other declared fields optional; undeclared required fields use the additional-property type, or `any` if inconsistent (implementation-defined; proposed) |
| `additionalProperties: false` | Closed record; no dynamic index signature |
| `additionalProperties: true` or omitted | Open record with `{[key: string]: any}` in addition to named fields |
| `additionalProperties: schema` | Index signature for additional keys only, whose value type is derived from that schema; named fields retain their own types |
| `patternProperties` | Patterns cannot be represented as literal keys; approximate dynamic values by the union of pattern value types and permitted additional-property types (implementation-defined; proposed) |
| `propertyNames` | Does not change value types; validate key constraints at call time |
| `dependentRequired`, `dependentSchemas` | Preserve base record shape where sound; cross-property rules validated at call time |
| `items: schema` | `T[]`, or the tuple tail if `prefixItems` exists |
| Missing `items`, or `items: true` | `any[]`, or an unrestricted `any` tuple tail |
| `items: false` | No tail after `prefixItems`; without a prefix, only the empty array passes validation |
| `prefixItems: [A, B]` | Tuple prefix `[A, B]`, with length/remaining items still subject to the schema |
| Draft-07 `items: [A, B]` | Tuple prefix, applying the older dialect's `additionalItems` |
| Draft-07 `additionalItems` | Tuple tail restriction, ignored when not applicable in that dialect |
| `contains`, `minContains`, `maxContains`, `uniqueItems` | Array type unchanged; validate at call time |
| `oneOf` | Union of alternatives; exclusive-match requirement validated at call time |
| `anyOf` | Union of alternatives |
| `allOf` | Merge compatible record constraints or other representable intersections; otherwise `any` (implementation-defined; proposed) |
| `not` | Cannot express general complements; retain an independently known base type or `any`, validate negation at call time |
| `if`, `then`, `else` | Approximate as the union of applicable branch shapes when sound, otherwise `any`; validate conditional rules at call time (implementation-defined; proposed) |
| `$ref` | Resolve local or already-available referenced schema, derive its target |
| `$defs`, older `definitions` | Named schema definitions; resolve references, not record fields |
| Recursive `$ref` | Render the referenced type by name instead of infinitely expanding it |
| `$id`, `$anchor` | Resolve schema identifiers/anchors; no independent value type |
| `$dynamicRef`, `$dynamicAnchor`, older `$recursiveRef`/`$recursiveAnchor` | Best-effort reference resolution; unresolved/dynamic cases degrade to `any` |
| `$schema`, `$vocabulary` | Dialect/vocabulary hints; unknown vocabularies cannot break derivation |
| `nullable: true` | Compatibility extension: add `null` to the derived type |
| `format`, `pattern` | Not part of the MCPL type; validated at call time according to the validator/dialect |
| `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf` | Numeric validation only |
| `minLength`, `maxLength`, `minItems`, `maxItems`, `minProperties`, `maxProperties` | Bounds are call-time validation constraints, not new type forms |
| `unevaluatedProperties`, `unevaluatedItems` | Best-effort open/closed shape approximation; applicator-sensitive rules validated at call time |
| `contentEncoding`, `contentMediaType`, `contentSchema` | No automatic decoding or new type; retain the base type and dialect-defined validation/annotation behavior |
| `title`, `description`, `default`, `examples`, `$comment` | Documentation/annotations; no default insertion or value coercion |
| `readOnly`, `writeOnly`, `deprecated` | Documentation annotations; no value transformation |
| Unknown keywords / unsupported constructs | `any` for unsupported type information; never fail derivation |

This table covers the standard validation/applicator/metadata families, not a
promise that arbitrary third-party vocabularies are understood. `format` assertion
configuration is **(implementation-defined; proposed: enable supported format
checks at call time; unsupported formats remain annotations)**.

JSON Schema keywords such as `properties` apply only to objects and do not imply
`type: "object"` under the standard. Structural inference without an explicit
type is **(implementation-defined; proposed: preserve non-object possibilities
rather than silently treating `properties` as an object assertion; use `any` if
the complete overapproximation is not expressible)**. The same applies to array
and numeric constraints without `type`.

`prefixItems` does not by itself require every position to exist, and `items`
does not prohibit a tail unless false. Tuple rendering is therefore
**(implementation-defined; proposed: use optional positions for nonrequired prefix
slots and a rest tail in type displays, e.g. `[string?, number?, ...any[]]`;
`minItems: 2` with `items: false` yields `[string, number]`. These are type-display
notations only, not source optional/spread syntax)**. Reading a possibly absent
position still risks the runtime missing-index error, not a silent null.

Recursive type names are **(implementation-defined; proposed: prefer the `$defs`
or `definitions` key, disambiguate collisions by schema path, and print definitions
once in full help)**. Example display:

```text
Node = {value: string, children?: Node[]}
```

Validation and derivation are separate: unresolved remote/file references can
produce a usable discovery entry whose attempted call fails with an unsupported
schema error. Reference loading is **(implementation-defined; proposed: resolve
only locally available schemas; never fetch network/file references on demand)**.
Draft-04 boolean `exclusiveMinimum`/`exclusiveMaximum` modify the associated
bound; newer numeric forms are interpreted according to their dialect. Older
`definitions` are honored rather than discarded for not being `$defs`. In
2020-12, applicable sibling keywords alongside `$ref` also constrain the value;
older dialects that ignore reference siblings are interpreted best-effort under
their own rules, not silently rewritten to modern semantics.

### Tool signatures and MCP content

Each discovered tool becomes a function taking one object argument:

```text
playwright.browser_navigate: (args: {url: string}) => Content[]
```

The illustrative signature above is a fixture, not a promise about every version
of Playwright's schema. Real signatures come from discovery, including optional
fields and open-record index signatures where appropriate.

`Content` is the MCP tool-result content-block union. The type definitions below
follow [MCP 2025-11-25's schema](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/schema/2025-11-25/schema.ts).
Choosing that revision as the baseline is **(implementation-defined; proposed)**;
the negotiated SDK protocol may add extension fields. `Meta` maps MCP `unknown`
to MCPL `any`; optional fields use MCPL optional-read semantics.

```typescript
Meta = {[key: string]: any}
Role = "user" | "assistant"
Annotations = {
  audience?: Role[],
  priority?: number,
  lastModified?: string
}
Icon = {
  src: string,
  mimeType?: string,
  sizes?: string[],
  theme?: "light" | "dark"
}
TextResourceContents = {
  uri: string,
  mimeType?: string,
  text: string,
  _meta?: Meta
}
BlobResourceContents = {
  uri: string,
  mimeType?: string,
  blob: string,
  _meta?: Meta
}
TextContent = {
  type: "text",
  text: string,
  annotations?: Annotations,
  _meta?: Meta
}
ImageContent = {
  type: "image",
  data: string,
  mimeType: string,
  annotations?: Annotations,
  _meta?: Meta
}
AudioContent = {
  type: "audio",
  data: string,
  mimeType: string,
  annotations?: Annotations,
  _meta?: Meta
}
EmbeddedResource = {
  type: "resource",
  resource: TextResourceContents | BlobResourceContents,
  annotations?: Annotations,
  _meta?: Meta
}
ResourceLink = {
  type: "resource_link",
  name: string,
  title?: string,
  uri: string,
  description?: string,
  mimeType?: string,
  size?: number,
  icons?: Icon[],
  annotations?: Annotations,
  _meta?: Meta
}
Content = TextContent | ImageContent | AudioContent | EmbeddedResource | ResourceLink
```

MCP base64, MIME, URI, timestamp, audience, and priority constraints remain
protocol/schema constraints rather than additional MCPL primitive types.
Image/audio data and resource blobs are base64 **strings**, not byte buffers.
Resource links are data; returning one does not cause an implicit resource read.
This union does not include sampling-only tool-use/tool-result blocks.

Call value selection uses these explicit branches:

0. If `isError: true`, raise a runtime error carrying the tool's text. Do not
   return a successful value, even if its content validates.
1. Otherwise, if the tool has an `outputSchema`:
   - If `structuredContent` is present, validate it against the schema and return
     it with the derived type. A validation failure is a runtime error; do not
     fall back to text.
   - Otherwise, if there is exactly one text block, parse that block as JSON,
     validate against the schema, and return the typed value. Invalid JSON or
     validation failure is a runtime error. Do not concatenate prose or strip
     fences.
   - Otherwise, raise a runtime error: no valid schema-backed result is available.
2. Otherwise, the tool has **no** `outputSchema`: return `content` as `Content[]`,
   including an empty list or image/audio/resource blocks. Do not parse text as
   JSON or use `structuredContent`, even if it happens to be present.

The raw `CallToolResult` envelope is not exposed. `text(blocks)` concatenates only
text blocks, in order, with `"\n"`; it does not include embedded resource text.
Tool error message extraction is **(implementation-defined; proposed: join error
text blocks with newlines, or use a clear generic message if none exist)**.

## 9. Type rules

### Static checking and gradual checks

The complete program is parsed and type-checked before evaluation. The checker
uses the discovered server environment and session binding types, not speculative
tool results. Earlier declarations in the same program are visible to later
statements; forward references are not. Structural assignability checks required
fields, optional fields, arrays, tuples, and every possible union alternative.
Extra-field handling is **(implementation-defined; proposed: reject known extra
fields against closed record types; accept them against an appropriate index
signature)**. JSON Schema remains the final authority at a tool boundary.

`any` is gradual, not an unchecked escape from runtime safety. It is accepted
where any type is expected; the operation then checks its actual value at
runtime. Member/index access on `any` yields `any`. Calls on an `any` callee must
check that the value is callable, its arity, and applicable parameter constraints.
Union-with-`any` rendering is **(implementation-defined; proposed: simplify to
`any`)**. Runtime checks fail with `runtime error`, not retroactive type errors.

### Operators

| Operator | Static operands | Result / behavior |
| --- | --- | --- |
| unary `-` | `number` | `number`; finite negation |
| `!` | `boolean` | `boolean` |
| `*`, `/`, binary `-` | `number`, `number` | `number` |
| `+` | `number`, `number` | `number` |
| `+` | `string`, `string` | `string` concatenation |
| `<`, `<=`, `>`, `>=` | Both numbers or both strings | `boolean` |
| `==`, `!=` | Overlapping JSON-capable types | `boolean`, deep structural comparison |
| `&&`, `||` | `boolean`, `boolean` | `boolean`, short-circuit evaluation |
| `c ? a : b` | `c: boolean` | Union of both branch types |

No truthiness or implicit coercion exists. `1 ? "yes" : "no"`, `!0`, and
`"a" + 1` are type errors. Use `str(1)` for explicit conversion. A static union
must satisfy the operation for all alternatives unless narrowed, or gradual
`any` checks must establish it at runtime.

`==` compares JSON structurally: numbers by numeric equality, strings exactly,
booleans/null by value, arrays by ordered elements, and records by keys and
values, not key order **(implementation-defined; proposed: object key order does
not affect equality)**. No coercion occurs. Provably disjoint types cannot be
compared: `1 == "1"` is a type error, not false. Functions, tools, and servers
cannot be compared, even to themselves. Equality visits are charged fuel.

Division by zero, overflow, or any non-finite numeric result is a runtime error.
The ternary evaluates only one branch, but **both branches are checked** and their
types unify to a union; neither is ignored because of a constant condition.
`&&` evaluates its right side only when its left is true; `||` only when its left
is false. Short-circuiting is **(implementation-defined; proposed)**, matching the
chosen JS-flavored forms without adopting JS truthiness.

### Arrays, tuples, records, and missing values

Ordinary array literals infer an array of the union of their element types
**(implementation-defined; proposed: use a contextual tuple type when available;
otherwise `["a", 1]` has `(string | number)[]`, not an inferred fixed tuple)**.
The empty array literal is assignable to any array type. Its context-free display
is **(implementation-defined; proposed: `any[]`)**.

Tuple indexing with a literal integer preserves the slot type: if
`entry: [string, number]`, `entry[0]` is `string` and `entry[1]` is `number`.
Nonliteral tuple indexes yield the union of slot/tail types
**(implementation-defined; proposed)**. Provably out-of-range literal tuple
indexes are type errors. Dynamic indexes and ordinary array bounds are checked
at runtime; indexes must be finite integral
numbers and nonnegative. No negative-from-end indexing is implied.

Declared optional properties read as `T | null`. This interacts with the general
missing-key crash rule; the interview did not explicitly distinguish absent
optionals from arbitrary absent keys. **(implementation-defined; proposed: a
statically declared optional property read normalizes absence to `null`; this
is the sole missing-key exception. A missing undeclared key, an `any` lookup,
and a missing dynamic index-signature key still fail)**. A present property
whose value is null also reads as null; MCPL has no `undefined` value.

Statically provable access failures, such as a missing property on a known
record or a literal index outside a tuple's range, are type errors; otherwise
missing-key and out-of-range failures are runtime errors. A string literal index
is the same known-property read as dot access. Named fields take precedence over
an index signature: `r.k` and `r["k"]` have the named field's read type when `k`
is declared. The signature covers only additional keys. Dynamic `r[k]` with
`k: string` has the union of the signature type and all named-field read types
(including `null` for optional reads). For example, with
`r: {name: string, [key: string]: number}`, `r.name` is `string`, a non-named
literal read `r["count"]` is `number`, and a dynamic read is `string | number`.
A missing additional key still raises a runtime error. Computed keys are
allowed for **indexing**, not for constructing object literals. Record key
indexes require strings, not implicitly stringified numbers. Duplicate literal
object keys are **(implementation-defined; proposed: last value wins, matching
common JSON parsers)**. Field values evaluate left-to-right.

### Lambdas and functions

Lambdas have mandatory parameter parentheses and one expression body. Parameters
receive contextual types from the expected function signature:

```mcpl
map([1, 2], (x) => x + 1);
```

Here `x` is `number`. A lambda bound without context has `any` parameters:

```mcpl
identity = (x) => x;
```

Its type is `(x: any) => any`; a later call does not retroactively retype the
binding. Lambdas capture lexical values, may be stored in records/arrays, and
can be passed to builtins. Arity is exact **(implementation-defined; proposed)**;
there are no optional/rest/default parameters in source. Function assignability
is **(implementation-defined; proposed: contravariant parameter types and
covariant results, with gradual checks for `any`)**.

There are no recursive declarations, loops, or forward references. A lambda
cannot capture its own new binding, which does not exist until its right-hand
side finishes. Calling a value of type `any` is allowed, so self-application is
legal:

```mcpl
f = (x) => x(x);
f(f);
```

`f` has type `(x: any) => any`. The call exhausts fuel; the earlier `f` binding
is kept. **Termination is guaranteed by the fuel budget, not by construction.**
Waiting for a downstream tool is separately unbounded by design.

### Bounded flow narrowing

Narrowing applies only inside `condition ? a : b` and `condition && e`, and only
to these recognized condition forms:

| Condition | True arm / RHS of `&&` | False arm of `?:` |
| --- | --- | --- |
| `x != null` | Remove `null` from `x` | Keep `null` |
| `x == null` | Keep `null` | Remove `null` |
| `null != x` | Remove `null` from `x` | Keep `null` |
| `x.k == "lit"` | Keep union variants with that literal discriminant | Exclude those variants |

Here `x` is a variable (including a sequence local or lambda parameter) or a
member/index path, not an arbitrary computation.
Index-path identity is **(implementation-defined; proposed: only identical paths
with literal indexes are stable narrowing targets; do not equate independently
evaluated computed-index expressions)**. Discriminant `k` is a known literal
field whose variants make the test meaningful; it is not a general predicate
solver. The narrowed fact is local to the branch, not a permanent type mutation.

No additional narrowing is promised: not `null == x`, `x !== null`, `!condition`,
`typeof`, `has(x, "k")`, `||`, arbitrary equality, calls, nested boolean algebra,
or facts propagated through a separately bound boolean. Parentheses around the
same recognized condition do not change it **(implementation-defined; proposed)**.
There is no `!==` operator at all. Narrowing `any` does not conjure a known record
shape; runtime checks still apply.

```mcpl
// Given user: {name?: string}:
user.name != null ? upper(user.name) : "anonymous";
user.name != null && starts_with(user.name, "A");

// Given result: {kind: "ok", value: number} | {kind: "error", message: string}:
result.kind == "ok" ? str(result.value) : result.message;
```

## 10. Evaluation semantics

1. Validate input/source limits, parse, reserve planned names in a preflight
   environment, and check the **entire** program. No tool call or binding commit
   can occur before this succeeds.
2. Evaluate statements and sequence items in source order. Object fields, array
   elements, and call arguments evaluate left-to-right. Operand order is
   **(implementation-defined; proposed: left-to-right)**, subject to short-circuiting.
3. Evaluate each statement's right-hand side or expression to a complete value.
   Check value/resource limits, then commit its binding before starting the next.
4. Record its typed preview. Preview truncation must not evaluate the expression
   again or call any downstream tool.
5. On a runtime error or evaluation/resource limit failure, stop immediately.
   The failed statement creates no binding; earlier completed bindings stay.
   The result lists their previews above the one error.

Calls inside `map`, `filter`, `reduce`, or any other lambda execute sequentially,
in collection order. There is no implicit batching, retry, parallel map, or
transaction across tools. Sorting-key evaluations are described in section 15.
A statement can have already performed external side effects before failing;
MCPL cannot roll them back. No automatic retry of a failed tool call is safe.

Out-of-range array indexes and missing runtime keys fail immediately, subject
to the declared-optional exception in section 9. Arithmetic division by zero and
non-finite results fail immediately. A tool failure stops the enclosing lambda,
builtin, statement, and run rather than becoming an error-shaped data value.
There is no catch/try construct in v0.1.

Client cancellation aborts the run and preserves bindings from completed
statements. The active statement is not committed. Forwarding MCP cancellation
is best-effort; a downstream may already have acted. Cancellation response
transport details are **(implementation-defined; proposed: follow MCP cancellation
semantics; do not manufacture a successful binding or require a response to a
cancelled request)**.

The same program with the same environment, values, and downstream results has
the same fuel behavior. Tool results and state changes are external inputs, not
a promise of replay determinism across live browser sessions.

## 11. Tool calls

`server.tool({...})` is a normal call to a first-class tool function. It takes
exactly **one object argument**, including `{}` for a no-fields tool. There are
no named arguments, positional tool-field arguments, or implicit default object.
An expression producing an object can be passed; an object literal is not
required.

Before sending the downstream request:

1. Statically check the object expression against the derived input type.
2. At runtime ensure it is JSON data, not a closure/server/tool capability.
3. Validate it with JSON Schema against the retained raw `inputSchema`.
4. Check the downstream-call budget and availability, then invoke the exact
   discovered server/tool name with the object as MCP `arguments`.

Validation applies even when all types were statically known. Integrality,
patterns, formats, bounds, unions' exclusive matching, and other schema rules
cannot be bypassed by derivation to `any`. Invalid schemas and unavailable
references cause a runtime schema-validation error, not an unvalidated call.
No schema defaults, coercions, or unknown-key stripping are applied
**(implementation-defined; proposed)**.

Result selection and validation follow section 8. The registry's classified
errors (unknown server/tool, unavailable, protocol/transport, invalid arguments,
invalid schema, invalid result, tool-reported failure) map to positioned runtime
errors in the calling statement. Known-name mistakes should usually have been
caught as type errors earlier. Runtime errors preserve the useful underlying
tool message without dumping unrelated large payloads.

The existing proxy core has deadline-taking APIs and a timeout classification.
Those are **not** the v0.1 language contract: execution/discovery must not impose
wall-clock limits. Integration must remove language-imposed deadlines rather
than emulate “unlimited” with a very large duration. A downstream's own reported
timeout is still a downstream failure, not an MCPL deadline.

## 12. The three MCP tools

Exactly `run`, `help`, and `show` are advertised by the language runtime.
Descriptions and schemas here are the target protocol surface; the current
proxy stub is not modified by this specification.

Result envelopes are **(implementation-defined; proposed: return one MCP text
content block containing the described text, with `isError: false` on success
and `isError: true` on errors; do not promise an additional `structuredContent`
shape)**. Protocol-invalid envelopes and unknown MCP method/tool names remain
protocol errors; language failures and invalid arguments to these three tools
use the error contract in section 13.

### `run`

**Description:** exactly the `RUN_DESCRIPTION` block in section 2.

**Input JSON Schema:**

```json
{
  "type": "object",
  "properties": {
    "program": {"type": "string"}
  },
  "required": ["program"],
  "additionalProperties": false
}
```

The program's byte limit is checked by the language rather than JSON Schema
`maxLength`, which counts characters. Output has one line per new binding:

```text
name: type (size) = preview
```

Size labels are **(implementation-defined; proposed: arrays/tuples use `N items`,
objects use `N keys`, strings use `N chars`, scalar numbers/booleans/null use
`1 value`, functions/tools use `function`, and servers use `N tools`; use singular
`item`, `key`, `char`, `tool` for one)**. Size describes the whole value, not its
preview, and string length counts Unicode scalar values.

Default preview limits:

| Dimension | Default |
| --- | --- |
| Expanded container depth | 2 |
| Array items per expanded array | 5 |
| Object keys per expanded object | 8 |
| String scalar values before truncation | 120 |
| Total display per binding | Approximately 600 characters |
| Complete run text output | At most 8 KB |

Depth counting is **(implementation-defined; proposed: the root container is
level 1; at depth 2 show children containers but collapse grandchildren to
`[…]` or `{…}`. Scalar leaves do not consume another container level)**.
Count-truncation markers are exact:

- Array: `…+10` means ten further items, e.g. `[1, 2, …+10]`.
- Object: `…+3 keys` means three further keys, e.g. `{a: 1, …+3 keys}`.
- String: `…` inside the closing quote, e.g. `"abc…"`. The size slot for a
  top-level string still reports its **full** scalar length.
- Empty arrays/objects are `[]`/`{}`, never misleading truncation markers.

The count limits are maxima, not guarantees of displaying that many items: the
per-binding character allowance can truncate earlier. Budget handling is
**(implementation-defined; proposed: reserve space for the name/type/size, then
render a balanced preview with exact omitted-item counts. If the type itself is
too large, shorten its display with `…`; `help`/`show` can disclose more. Count
ellipsis and escaping in the output budget; never split a Unicode scalar)**.
`…` markers are disclosure notation, not valid source literals.

Example, assuming the other ten array items do not fit the character budget:

```text
tabs: {title: string, url: string}[] (12 items) = [{title: "GitHub", url: "https://git…"}, {title: "Docs", url: "https://docs…"}, …+10]
```

A hard 8 KB output cap cannot always contain a line for every new binding.
**(implementation-defined; proposed: retain all committed bindings, emit as many
lines as fit, and end with `…+N bindings (output limit; use show)`; for a failed
run reserve room for its diagnostic before fitting binding lines)**. Disclosure
truncation is not evaluation failure. A short type/error message is bounded as
needed, but must preserve its category, failing statement position, and hint.

### `help`

**Exact description:**

```text
Discover MCPL servers, tools and built-ins without evaluating a program. Omit topic, or pass an empty string, for servers and built-in groups. Pass a server name for its tools, server.tool for a tool's full signature and description, or a built-in name for its signature and documentation. Output is not preview-truncated, but is capped at 64 KB. This tool creates no bindings.
```

**Exact input JSON Schema:**

```json
{
  "type": "object",
  "properties": {
    "topic": {"type": "string"}
  },
  "additionalProperties": false
}
```

| Topic | Output |
| --- | --- |
| Absent or `""` | Configured servers and availability; builtin groups and their names |
| Exact server name | Tool names/signatures with one-line descriptions |
| `"server.tool"` | Full tool signature and full downstream description |
| Exact builtin name | Signature(s), documentation, and edge cases |

All output is fully rendered, not subject to run preview depth/item/string limits,
up to 64 KB. Description line splitting, headings, ordering, and cap marker are
**(implementation-defined; proposed: deterministic lexical ordering of server/tool
names, headings `Servers` and `Built-ins` for the overview, and final `… (output
limit)` when the cap truncates text)**. Preserve raw downstream description text
in the detail view. Descriptions are documentation, not instructions to execute.

Topic lookup is not expression evaluation. Dotted topics need a policy
**(implementation-defined; proposed: prefer exact builtin/server matches; otherwise
split at the longest configured server prefix followed by `.`, and match the
entire remaining tool name verbatim. Thus `s.tool..name` works for help even when
it cannot be a language symbol)**. Unknown topics produce a type-category error
with a suggested valid topic where possible. `help` neither creates a binding
nor updates binding LRU ages.

### `show`

**Exact description:**

```text
Evaluate one MCPL expression against this session and display its value without creating a binding. Use stored names and field or index access to inspect results. Optional depth, max_items, max_string and max_output control disclosure; output is capped at 64 KB. Binding lookups count as use for eviction. Expressions may call tools and have their usual effects. Functions, servers and tools display only their type.
```

**Exact input JSON Schema:**

```json
{
  "type": "object",
  "properties": {
    "expression": {"type": "string"},
    "depth": {"type": "integer", "minimum": 0, "maximum": 64},
    "max_items": {"type": "integer", "minimum": 0, "maximum": 100000},
    "max_string": {"type": "integer", "minimum": 0, "maximum": 16777216},
    "max_output": {"type": "integer", "minimum": 256, "maximum": 65536}
  },
  "required": ["expression"],
  "additionalProperties": false
}
```

`max_output` has a minimum of 256 bytes. Other integral bounds in this schema
are **(implementation-defined; proposed: accept only nonnegative integral
disclosure counts within the hard value/output limits)**. This refines the
interview's number-valued optional parameters without allowing fractional depths
or lengths.

`show` parses exactly one `Expression`, not a program: no top-level assignment or
statement sequence. Bracket-delimited sequences and locals are permitted inside
that expression. Semicolon handling is **(implementation-defined; proposed: no
trailing semicolon; comments/whitespace around the expression are allowed)**.
All static, runtime, schema, fuel, and value checks still apply. It creates no
session binding, does not consume an automatic name, and global lookups count
toward LRU. It may invoke tools because it evaluates an expression, not a
restricted path language.

Disclosure defaults are **(implementation-defined; proposed: depth 64,
max_items 100000, max_string 16777216, max_output 65536; `max_items` limits both
array entries and object keys)**. Caller-selected disclosure caps apply only to
the rendered value; diagnostics ignore those caps and are limited only by the 64 KB
hard cap. The selected value output cap is hard and cannot exceed 64 KB. Depth
zero collapses containers; type-only values display their type regardless of
depth. Normal JSON-like value rendering uses the same truncation markers as
`run`, without a fabricated binding name. If even the collapsed top-level form
cannot fit the output budget, render only its type and size; the type may be
shortened to fit. Top-level string output includes its size
**(implementation-defined; proposed: prefix `string (N chars) =` when truncated so
full length is not lost)**.

Example MCP arguments and response text:

```json
{"expression":"res1[0]","depth":3,"max_items":20,"max_string":500,"max_output":4096}
```

```text
{title: "Example", url: "https://example.com"}
```

Fuel/call/source limits were phrased per `run`; otherwise `show` would bypass
all execution guardrails. **(implementation-defined; proposed: give each `show`
the same fresh execution budgets as a run, but never a binding commit)**.

## 13. Errors

Four language error categories exist:

| Category | Phase and policy |
| --- | --- |
| `syntax error` | First parse/lexical error only; execute nothing |
| `type error` | All independent type errors, up to 10; execute nothing |
| `runtime error` | One error; stop and keep earlier completed bindings |
| `limit exceeded` | One error; stop and keep earlier completed bindings |

Every error result has MCP `isError: true`. Earlier binding lines precede a
runtime/limit diagnostic. A limit found before evaluation, such as source size,
commits nothing. The caller must not infer rollback from `isError`.

Rust-style layout:

```text
category: message
 --> line:col
  |
N | source line
  | ^^^ underline
  hint: optional corrective action
```

Source locations, span formatting, and exact wording beyond approved examples
are **(implementation-defined; proposed: 1-based lines and Unicode-scalar columns;
underline the smallest relevant expression; print the first line of a multiline
span and mark continuation; emit type errors in source order)**. Very long source
lines must be clipped to fit output limits without losing the indicated column.
For invalid MCP input objects with no source, use the field name in place of a
source excerpt **(implementation-defined; proposed)**.

The following concrete messages are proposed stable diagnostic templates. Each
example is independent unless it explicitly shows committed bindings.

### Missing statement terminator

Source `x = 1`:

```text
syntax error: expected ';' after statement
 --> 1:6
  |
1 | x = 1
  |      ^
  hint: every statement must end with ';'
```

### Binary operator without whitespace

```text
syntax error: binary operator '==' requires whitespace on both sides
 --> 1:2
  |
1 | a==b;
  |  ^^
  hint: add whitespace on both sides of '=='
```

### Sequence without enclosing brackets

```text
syntax error: unexpected '='
 --> 1:7
  |
1 | x = a = 1; a + 1;
  |       ^
  hint: to sequence inside an expression, wrap it in parentheses: (a = 1; a + 1)
```

### Unknown name with suggestion

```text
type error: unknown name 'playwrigt'
 --> 1:1
  |
1 | playwrigt;
  | ^^^^^^^^^
  hint: did you mean 'playwright'?
```

### Redefinition

Assume `res2` was previously committed:

```text
type error: res2 is already defined
 --> 1:1
  |
1 | res2 = 32;
  | ^^^^
  hint: choose a new name; session bindings cannot be reassigned
```

### Wrong tool argument type

```text
type error: argument field 'url': expected string, found number
 --> 1:35
  |
1 | playwright.browser_navigate({url: 123});
  |                                   ^^^
  hint: use a URL string, for example "https://example.com"
```

### Missing static property

Given `record: {x: number}`:

```text
type error: property 'missing' does not exist on {x: number}
 --> 1:8
  |
1 | record.missing;
  |        ^^^^^^^
  hint: available field: x
```

### Non-boolean condition

```text
type error: conditional requires boolean, found number
 --> 1:1
  |
1 | 1 ? "yes" : "no";
  | ^
  hint: compare explicitly; MCPL has no truthiness
```

### Runtime out-of-range index

```text
runtime error: array index 4 out of range for length 2
 --> 1:1
  |
1 | [10, 20][4];
  | ^^^^^^^^^^^
  hint: valid indexes are 0 through 1
```

### Tool-reported failure

```text
runtime error: playwright.browser_navigate: net::ERR_NAME_NOT_RESOLVED
 --> 1:1
  |
1 | playwright.browser_navigate({url: "https://invalid.example"});
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  hint: inspect the URL and retry the call as a separate program
```

### Fuel exhaustion

For a fresh run containing `xs = range(100000);` followed by the shown statement:

```text
xs: number[] (100000 items) = [0, 1, 2, 3, 4, …+99995]
limit exceeded: evaluation budget exhausted (10,000,000 units), statement 2
 --> 2:1
  |
2 | map(xs, (x) => sum(xs));
  | ^^^^^^^^^^^^^^^^^^^^^^
  hint: reduce the data size or split the program
```

### Evicted binding

```text
runtime error: `res3` was evicted (session memory limit); recompute it
 --> 1:1
  |
1 | res3[0];
  | ^^^^
  hint: rerun the computation under a new name
```

## 14. Guardrails

**There are no wall-clock limits anywhere in the MCPL contract:** no program
deadline, per-call timeout, discovery/connect deadline, or elapsed-time CPU
budget. Downstream MCP calls may legitimately take arbitrarily long. Waiting
must cost no fuel and MCPL must not stand in the way. Explicit client cancellation
still works; external transport failures and downstream-imposed limits can still
be reported. This differs from the current proxy core's deadline-based APIs.

All limits below are compiled-in defaults, not user-configurable v0.1 options.
Byte units are **(implementation-defined; proposed: KB = 1024 bytes and MB =
1024 × 1024 bytes, measured as UTF-8 for source/string/output limits)**. Character
positions and string disclosure counts remain Unicode scalar counts.

| Resource | Default | Enforcement |
| --- | --- | --- |
| Evaluation fuel | 10,000,000 units per run | Before charged work; exhausted budget stops the active statement |
| Downstream calls | 50 per run | Before starting call 51, including calls inside lambdas |
| Program source | 64 KB | Before parsing |
| Elements per array/object | 100,000 | Before allocating/growing beyond the limit; includes `range` |
| String length | 16 MB | Decoded value bytes; includes construction, parsing, and tool output |
| Session memory | 256 MB | LRU eviction, then failure if space cannot be made |
| Run output | 8 KB | Disclosure truncation, not value mutation |
| Help output | 64 KB | Full rendering until the cap, then a truncation marker |
| Show output | At most 64 KB | Caller may request less; never more |
| Value nesting depth | 64 | Check literals, constructed values, parsed JSON, and tool results |
| Expression nesting depth | 256 | Check parser/AST nesting before evaluation |

A limit failure names the limit and statement, with a corrective hint. Output
caps instead truncate disclosure as section 12 specifies; otherwise large valid
bindings would become unusable merely because their preview did not fit.
A source/expression limit detected before statements can be parsed identifies
the source position or whole program **(implementation-defined; proposed)**.

Depth accounting is **(implementation-defined; proposed: value depth counts
nested arrays/records, starting at 1 for a root container; expression depth counts
nested AST expressions, starting at 1. Implementation recursion stacks must also
be protected by bounded evaluation rather than unguarded host recursion)**.
Capabilities and their captures count toward memory, but are not recursively
expanded into JSON depth. Limits are checked before oversized allocation where
possible; malformed/oversized downstream payloads fail rather than evade limits.

### Deterministic fuel model

Each run starts with 10,000,000 units. Charges are deterministic, independent of
elapsed time:

- One unit per evaluated expression node, including each lambda-body invocation.
  Sequence items cost like any expression node; locals do not bypass fuel.
- Constructing or copying a collection costs its element count.
- String concatenation, substring, and regex work charge for characters touched.
- `sort_by` charges `n · log n` work, plus key-function evaluation.
- Builtins charge their input size plus per-element lambda evaluation where used.
- Each downstream tool call costs a fixed nominal amount regardless of duration;
  the interview's example amount was 10 units. Waiting costs **zero**.

Exact rounding and overlap between these charges are **(implementation-defined;
proposed: use 10 units per downstream invocation; for sorting use
`n * ceil(log2(max(n, 1)))`; add expression, builtin input-scan, allocation/copy,
and lambda costs rather than treating any as replacements. Charge deep JSON
traversals by visited nodes plus touched string scalars; charge serialization,
validation, equality, and regex work so native operations cannot bypass fuel)**.
Call budget counts requests actually sent, including ones that fail downstream;
validation failures before sending do not consume a call
**(implementation-defined; proposed)**.

A short-circuit scan may charge only the inspected prefix
**(implementation-defined; proposed)**. Fuel must be charged incrementally or
precharged before native work; checking only after a large allocation/computation
is insufficient. Linear-time regex avoids catastrophic backtracking but still
needs proportional charges. Fuel exhaustion's core message is:

```text
limit exceeded: evaluation budget exhausted (10,000,000 units)
```

Append the statement number and source location, and hint to reduce data or split
the program. Different downstream results can change work; equivalent evaluation
on identical inputs must not fail randomly because of CPU scheduling.

## 15. Standard library

These are the entire v0.1 builtin set. Functions are called as `map(xs, f)`, not
`xs.map(f)`. `help` and `show` are **not** builtins. Types `T`, `U`, `A`, `B`, and
`K` below are documentation metavariables; `K` is `number` or `string`.
Signatures with `?` describe optional builtin arguments, not source parameter
annotations. Each builtin name is permanently reserved at session scope; lexical
parameters and locals may shadow it.

Unless an edge behavior was decided explicitly, the following tables' exact
arity, defaults, and edge policies are **(implementation-defined; proposed)**.
They give implementers a consistent minimal surface, not permission to silently
import the rest of JavaScript's standard library. All operations enforce the
value/fuel limits; integer indices/counts are checked at runtime when necessary.

### Collections

| Signature | Documentation and edge behavior |
| --- | --- |
| `len(xs: T[]) => number`; `len(s: string) => number`; `len(o: {[key: string]: T}) => number` | Count elements, Unicode scalars, or keys; tuples count positions. Empty input returns 0. |
| `map(xs: T[], f: (x: T) => U) => U[]` | Apply `f` once per element, sequentially in input order. Empty input returns `[]`. |
| `filter(xs: T[], f: (x: T) => boolean) => T[]` | Keep elements whose predicate is true, preserving order. No predicate truthiness or automatic union narrowing. |
| `reduce(xs: T[], f: (acc: U, x: T) => U, init: U) => U` | Left fold starting at mandatory `init`; empty input returns `init`. The argument order is `xs, f, init`. |
| `flat_map(xs: T[], f: (x: T) => U[]) => U[]` | Map and concatenate one array level, in order. Every callback must return an array. |
| `find(xs: T[], f: (x: T) => boolean) => T \| null` | First matching element, or `null`; stop calling the predicate after a match. |
| `any(xs: T[], f: (x: T) => boolean) => boolean` | True at the first match; false on empty input; short-circuits. |
| `all(xs: T[], f: (x: T) => boolean) => boolean` | False at the first failure; true on empty input; short-circuits. |
| `sort_by(xs: T[], f: (x: T) => K) => T[]` | Stable ascending sort by number or string keys; evaluate each key once in input order before sorting. Keys in one sort must all have the same primitive kind; mixed kinds through `any` fail. |
| `reverse(xs: T[]) => T[]` | Return the elements in reverse order without mutating input. |
| `slice(xs: T[], start: number, end?: number) => T[]` | Half-open slice `[start, end)`; default end is length. Proposed: integral nonnegative bounds, clamp to length, end before start yields `[]`. |
| `concat(xs: T[], ys: U[]) => (T \| U)[]` | Concatenate exactly two arrays, preserving order. No implicit flattening beyond the two inputs. |
| `unique(xs: T[]) => T[]` | Keep first occurrences using deep JSON equality, in order. Non-JSON elements fail as equality does. |
| `group_by(xs: T[], f: (x: T) => string) => {[key: string]: T[]}` | Group by exact string key, preserving element order within each group; no implicit conversion of numeric keys. |
| `zip(xs: A[], ys: B[]) => [A, B][]` | Pair elements by position; proposed: stop at the shorter input. |
| `range(n: number) => number[]`; `range(start: number, end: number) => number[]` | Produce unit-step integers from 0 to `n`, or start to end, excluding end. Proposed: finite integral bounds; descending/equal bounds yield `[]`; negative one-argument `n` fails. Enforce 100,000 items before allocation. |
| `sum(xs: number[]) => number` | Add in input order; `sum([])` is 0. Non-finite results fail. |
| `min(xs: number[]) => number` | Smallest number; empty input is a runtime error. |
| `max(xs: number[]) => number` | Largest number; empty input is a runtime error. |
| `first(xs: T[]) => T` | First element; empty input is a runtime error. |
| `last(xs: T[]) => T` | Last element; empty input is a runtime error. |

Callbacks receive only the documented arguments, not an implicit index or
collection. Inputs are immutable; builtins always return new values or existing
immutable elements. Tuple inputs are acceptable where arrays of their slot-union
type are expected **(implementation-defined; proposed)**.

Stable sorting means equal keys preserve input order. `group_by` always returns
a string-keyed record, never a special map value. A large nested `map` is still
bounded by fuel and output-value limits even when each individual input array
is within the element cap.

`range` needs representable increments with f64
**(implementation-defined; proposed: require bounds within the exact safe-integer
range, −(2^53 − 1) through 2^53 − 1, so adding one always progresses)**.

### Objects

| Signature | Documentation and edge behavior |
| --- | --- |
| `keys(o: {[key: string]: T}) => string[]` | Return the object's own keys. Proposed: insertion order. |
| `values(o: {[key: string]: T}) => T[]` | Return values in the same order as `keys`. |
| `entries(o: {[key: string]: T}) => [string, T][]` | Return key/value tuples in that order. |
| `from_entries(xs: [string, T][]) => {[key: string]: T}` | Build a record; proposed: later duplicate keys replace values without moving the original key's position. |
| `merge(a: {[key: string]: T}, b: {[key: string]: U}) => {[key: string]: T \| U}` | Shallow two-object merge; proposed: right-hand values win on duplicate keys. No recursive merge. |
| `has(o: {[key: string]: T}, key: string) => boolean` | Test key presence without reading its value; a present null counts as present. Does not narrow types. |

Known record inputs are accepted by these signatures; heterogeneous records
produce a union of property value types **(implementation-defined; proposed)**.
Optional absent properties are not fabricated as keys by `keys`/`entries`.
Records have no prototypes; keys such as `"__proto__"` are ordinary data.
JSON-decoded and tool-result object ordering is **(implementation-defined;
proposed: preserve input order when available, otherwise use lexical key order
consistently)**. No portable program should treat external object key order as
semantic data; sort explicitly if order matters.

### Strings

| Signature | Documentation and edge behavior |
| --- | --- |
| `str(value: any) => string` | Explicit textual conversion. Proposed: identity for strings, numeric/boolean/null display for scalars, compact JSON for JSON containers, type display for non-JSON values. |
| `num(s: string) => number` | Parse a finite number. Proposed: require the whole string to match JSON number grammar, without surrounding whitespace; failure is a runtime error. |
| `split(s: string, separator: string) => string[]` | Literal split preserving empty fields. Proposed: empty separator yields Unicode scalar strings; empty input with empty separator yields `[]`. |
| `join(xs: string[], separator: string) => string` | Join with separator between elements; empty input yields `""`. No element coercion. |
| `trim(s: string) => string` | Remove leading/trailing whitespace. Proposed: Unicode White_Space characters. |
| `lower(s: string) => string` | Unicode lowercase without locale selection; may change length. |
| `upper(s: string) => string` | Unicode uppercase without locale selection; may change length. |
| `contains(s: string, part: string) => boolean`; `contains(xs: T[], value: T) => boolean` | Literal substring search, or deep JSON membership. Empty substring is always present. Non-JSON array equality is rejected. |
| `starts_with(s: string, prefix: string) => boolean` | Exact prefix test; empty prefix succeeds. |
| `ends_with(s: string, suffix: string) => boolean` | Exact suffix test; empty suffix succeeds. |
| `replace(s: string, from: string, to: string) => string` | Proposed: replace all nonoverlapping literal matches, not regex; empty `from` is a runtime error. Replacement text is literal. |
| `substring(s: string, start: number, end?: number) => string` | Half-open Unicode-scalar slice. Proposed: integral nonnegative bounds, default end length, clamp to length, end before start yields `""`. |
| `index_of(s: string, part: string) => number` | First substring position in Unicode scalars; proposed: −1 if absent, 0 for empty substring. |
| `lines(s: string) => string[]` | Proposed: split LF/CRLF, remove separators, omit the final empty item from a terminal newline; empty input returns `[]`. A lone CR is retained as text. |
| `regex_match(s: string, pattern: string) => boolean` | Use raw strings for regex patterns. Proposed: true if any substring matches; use anchors for whole-string matching. Invalid regex is a runtime error. |
| `regex_find_all(s: string, pattern: string) => string[][]` | Use raw strings for regex patterns. All nonoverlapping matches, each containing the full match first, then capture groups. No matches yields `[]`. |
| `regex_replace(s: string, pattern: string, replacement: string) => string` | Use raw strings for regex patterns. Proposed: replace all nonoverlapping regex matches with literal replacement text; no replacement interpolation syntax in v0.1. |

Regexes use a **linear-time** engine. Dialect and unmatched-capture policy are
**(implementation-defined; proposed: Rust `regex` syntax, Unicode enabled;
no backreferences or look-around; unmatched optional captures become `""` to
preserve `string[][]`. Empty matches advance by a Unicode scalar, allowing one
terminal empty match, so scanning always terminates)**. Use raw strings for regex
patterns: `r"\d+"` passes the backslash directly to the regex engine. Normal
strings process JSON escapes first: `"\bRust\b"` contains backspaces and
`"\d"` is a syntax error, not a digit-class pattern.
Unicode case/whitespace tables are **(implementation-defined; proposed: the
implementation's pinned Unicode data version, reported by build documentation)**.

### Math

| Signature | Documentation and edge behavior |
| --- | --- |
| `floor(x: number) => number` | Greatest integral number not greater than `x`. |
| `ceil(x: number) => number` | Least integral number not less than `x`. |
| `round(x: number) => number` | Nearest integral number; proposed: ties away from zero. |
| `abs(x: number) => number` | Absolute value; result must remain finite. |

These do not introduce an integer type. All take and return f64 values.

### JSON and MCP

| Signature | Documentation and edge behavior |
| --- | --- |
| `parse_json(s: string) => any` | Parse strict JSON only, not MCPL bare keys/trailing commas/comments. Invalid JSON or non-finite numbers fail. Enforce all value limits. Tools without `outputSchema` return `Content[]`; `parse_json(text(r))` parses them when the text is JSON. If JSON is wrapped in other text, isolate it first (`substring`, `split`, `index_of`); the error identifies the JSON bounds. |
| `to_json(value: any) => string` | Compact JSON for JSON values; functions/servers/tools render their type, not an executable encoding. Nested non-JSON policy is in section 7. |
| `text(blocks: Content[]) => string` | Concatenate only `type: "text"` blocks with `"\n"`. No text blocks yields `""`; do not decode images/audio/resources. |

Serializer key ordering and escape choices are **(implementation-defined;
proposed: preserve the object's key order, emit legal compact JSON, escape
control characters and quotes, preserve other Unicode scalars)**. `parse_json`
is deliberately typed `any`; runtime checks or a tool schema must establish
stronger requirements. No builtin performs filesystem/network I/O.

`parse_json` remains strict: diagnostics do not extract JSON or accept surrounding
non-whitespace text. Bounds in diagnostics are **zero-based Unicode scalar
positions in the original string**, not UTF-8 byte offsets or program-source
positions. Spans `M..N` are half-open, matching `substring(s, M, N)`; leading
whitespace counts toward both bounds. Diagnostic excerpts are escaped, limited
to about 40 characters, and marked with `…` when truncated.

- **Trailing data:** when a complete JSON value starts after optional whitespace
  but non-whitespace text follows, report `invalid JSON: a complete JSON value
  ends at character N, followed by "<excerpt>"`. Hint: `isolate the JSON before
  parsing, e.g. substring(s, 0, N), or split on the text that follows it`.
- **Leading data:** when the string does not start with a JSON value after
  optional whitespace, report `invalid JSON: the string starts with
  "<excerpt>", not a JSON value`. Probe only the first `{` or `[` in the string.
  If a complete value parses there, hint `a JSON value spans characters M..N;
  try substring(s, M, N)`. Otherwise, hint `the first { is at character M; try
  substring(s, M)` (or `[` as appropriate). This is a candidate, not a promise
  that the substring parses; do not search later candidates. If neither marker
  exists, hint `expected a JSON value: {, [, ", a number, true, false or null`.
- **Malformed JSON:** retain the precise parser error and identify its position
  within the input string, rather than suggesting a generic retry.

The leading-data probe makes at most one extra parse attempt. Scanning and
probing consume fuel under the usual resource limits.

### Meta

| Signature | Documentation and edge behavior |
| --- | --- |
| `type_of(value: any) => string` | Return a TypeScript-style type display. Proposed: use runtime value shape for gradual data, declared signature for capabilities; do not expose memory addresses or closure internals. |

`type_of` does not itself narrow the caller's static type. Type displays are
strings, not an additional first-class type-reflection object model.

## 16. Worked examples

These are complete programs, not fragments with implicit source declarations.
Each example starts a fresh MCP session unless it says otherwise. Expected output
uses the proposed display conventions above. Downstream results are explicitly
fixed fixtures: a live browser/server can return different data and schemas.
The examples demonstrate the target contract, not an executed end-to-end test of
the current stub.

### A. First-contact discovery

Assume configuration names a server `playwright` whose fixture tool index has
exactly these two tools, with closed input records:

```text
browser_navigate: (args: {url: string}) => Content[]
browser_snapshot: (args: {}) => Content[]
```

Call the MCP `help` tool with `{}` first if no server name is known. Its proposed
text includes the server and the six builtin groups. Then call `run` with:

```mcpl
playwright;
playwright.browser_navigate;
```

Expected `run` output:

```text
res1: {browser_navigate: (args: {url: string}) => Content[], browser_snapshot: (args: {}) => Content[]} (2 tools) = {browser_navigate: (args: {url: string}) => Content[], browser_snapshot: (args: {}) => Content[]}
res2: (args: {url: string}) => Content[] (function) = (args: {url: string}) => Content[]
```

No browser call was made. To read the description in full, call MCP `help`:

```json
{"topic":"playwright.browser_navigate"}
```

For a fixture description `Navigate to a URL.`, the proposed output is:

```text
playwright.browser_navigate: (args: {url: string}) => Content[]
Navigate to a URL.
```

A real server with many tools has a truncated run type listing; `help` is the
full-signature path rather than dumping every schema into the initial prompt.

### B. Playwright aggregation across runs, then drill-down

Use the same two fixture signatures, but start a **new session**, so automatic
names begin again at `res1`. The fixture's initial navigation returns an empty
content list. Its first snapshot is one text block containing exactly:

```text
Page title: Index
[A](https://e.test/a)
[B](https://e.test/b)
[C](https://e.test/c)
[D](https://e.test/d)
[E](https://e.test/e)
[F](https://e.test/f)
```

There is no trailing newline. Subsequent navigation to each link returns `[]`;
its snapshot's text is `Page title: A`, `Page title: B`, and so on. These are
controlled illustrative snapshot strings; actual Playwright snapshot formatting
is version-dependent, so inspect it before choosing a regex.

**Run 1 — navigate once and keep only snapshot text:**

```mcpl
playwright.browser_navigate({url: "https://e.test"});
text(playwright.browser_snapshot({}));
```

Expected output (the snapshot is 149 Unicode scalars; escaped newlines do not
change its value length):

```text
res1: Content[] (0 items) = []
res2: string (149 chars) = "Page title: Index\n[A](https://e.test/a)\n[B](https://e.test/b)\n[C](https://e.test/c)\n[D](https://e.test/d)\n[E](https://e.…"
```

**Run 2 — extract links, keep five, visit sequentially, collect titles:**

```mcpl
regex_find_all(res2, r"https://e[.]test/[a-z]");
slice(res3, 0, 5);
map(res4, (link) => (
  playwright.browser_navigate({url: link[0]});
  {
    url: link[0],
    title: regex_find_all(
      text(playwright.browser_snapshot({})),
      r"Page title: ([^\n]+)"
    )[0][1]
  }
));
```

The parenthesized sequence completes navigation before constructing the record
and taking the snapshot. The navigation value is discarded, not committed as a
session binding. In the raw pattern, `\n` is a regex escape for newline, not an
MCPL string escape.

An equivalent replacement for the `map` statement uses a nested sequence with a
local snapshot string:

```mcpl
map(res4, (link) => (
  playwright.browser_navigate({url: link[0]});
  (snap = text(playwright.browser_snapshot({})); {
    url: link[0],
    title: regex_find_all(snap, r"Page title: ([^\n]+)")[0][1]
  })
));
```

Use either variant, not both; `snap` is local to each invocation and creates no
output binding. Both have the same expected output and downstream-call order.

Expected output:

```text
res3: string[][] (6 items) = [["https://e.test/a"], ["https://e.test/b"], ["https://e.test/c"], ["https://e.test/d"], ["https://e.test/e"], …+1]
res4: string[][] (5 items) = [["https://e.test/a"], ["https://e.test/b"], ["https://e.test/c"], ["https://e.test/d"], ["https://e.test/e"]]
res5: {url: string, title: string}[] (5 items) = [{url: "https://e.test/a", title: "A"}, {url: "https://e.test/b", title: "B"}, {url: "https://e.test/c", title: "C"}, {url: "https://e.test/d", title: "D"}, {url: "https://e.test/e", title: "E"}]
```

Run 2 performs ten downstream calls: one navigation then one snapshot for each
of five links. They are not concurrent. No raw snapshot passes through the agent
between the per-link navigation and extraction.

**Run 3 — drill into the stored aggregate:**

```mcpl
res5[0];
```

```text
res6: {url: string, title: string} (2 keys) = {url: "https://e.test/a", title: "A"}
```

**MCP `show` — read an existing expression without another binding:**

```json
{"expression":"res5[1]","depth":3,"max_items":20,"max_string":500,"max_output":4096}
```

```text
{url: "https://e.test/b", title: "B"}
```

The next expression statement would still receive `res7`, not `res8`; `show`
did not allocate a handle. It refreshed `res5`'s LRU age. If a snapshot lacks a
matching title, `[0]` fails and the map statement creates no `res5`; already
committed `res3` and `res4` remain and the browser side effects remain external.

### C. Optional-property and discriminated-union narrowing

A fixture server `catalog` exposes:

```text
profile: (args: {}) => {name?: string}
lookup: (args: {}) => {kind: "ok", value: number} | {kind: "error", message: string}
```

These types come from output schemas, not user annotations. In this example
`profile` returns structured content `{name: "Ada"}`, and `lookup` returns
`{kind: "ok", value: 42}`. The schemas disallow additional properties.

Complete program:

```mcpl
profile = catalog.profile({});
label = profile.name != null ? upper(profile.name) : "anonymous";
starts_a = profile.name != null && starts_with(profile.name, "A");
result = catalog.lookup({});
description = result.kind == "ok" ? str(result.value) : result.message;
```

Expected output:

```text
profile: {name?: string} (1 key) = {name: "Ada"}
label: string (3 chars) = "ADA"
starts_a: boolean (1 value) = true
result: {kind: "ok", value: number} | {kind: "error", message: string} (2 keys) = {kind: "ok", value: 42}
description: string (2 chars) = "42"
```

If the profile omits `name`, the declared-optional read yields null under the
proposed policy, `label` becomes `"anonymous"`, and `starts_a` is false without
calling `starts_with`. If lookup returns the error variant, only its `message`
is read. Neither fact requires general predicate analysis or optional chaining.

### D. Runtime failure at statement 3

Complete program in a fresh session:

```mcpl
good = [10, 20];
total = sum(good);
good[9];
unreachable = 0;
```

The whole program passes static checks because `good` is `number[]`, not a
fixed-length tuple. Expected result text, with MCP `isError: true`:

```text
good: number[] (2 items) = [10, 20]
total: number (1 value) = 30
runtime error: array index 9 out of range for length 2
 --> 3:1
  |
3 | good[9];
  | ^^^^^^^
  hint: valid indexes are 0 through 1
```

`good` and `total` remain available. The failed expression statement has no
committed automatic binding, and `unreachable` is not bound. A later program
may use a fresh name to retry the computation. Under the proposed automatic-name
policy, the next successful bare expression still receives `res1`.

Had statement 4 instead redefined `good`, preflight would reject the **entire**
program and neither earlier statement would run. Static failure is not a partial
transaction; runtime failure is partial progress.

### E. Locals and raw regex strings

Complete program in a fresh session:

```mcpl
(source = "Rust 123"; matches = regex_find_all(source, r"\d+"); {
  match: matches[0][0],
  count: len(matches)
});
```

Expected `run` output:

```text
res1: {match: string, count: number} (2 keys) = {match: "123", count: 1}
```

Only `res1` persists. `source` and `matches` are lexical locals; the raw pattern
passes `\d+` unchanged to the regex engine.

## 17. Open questions and deferred work

- General recursion and convergence analysis beyond fuel-based termination.
- A null-defaulting operator such as `??`; it is not v0.1 syntax.
- Parallel map or other parallel downstream execution.
- Proxying MCP resources and prompts in addition to tool calls.
- Refresh on `tools/listChanged` / `tools/list_changed`, reconnection, and the
  effect of changing discovered schemas on existing session types.
- Configurable limits; current defaults are compiled in.
- Preview tuning after the end-to-end demo, without changing stored values.

### Implementation-defined choices to confirm

The labelled proposals above are deliberately visible. Important integration
boundaries, rather than hidden language additions, are:

1. Exact lexer/PEG details, associativity, malformed Unicode treatment, symbol
   handling for configured server names, and duplicate-key policy.
2. Automatic-name allocation on collisions/failures, closure capture accounting,
   session serialization, and LRU byte accounting.
3. Conservative schema approximations, tuple tail/optional display, reference
   resolution, dialect/format validation, and the MCP content-schema baseline.
4. Absent optional fields versus arbitrary missing-key crashes; literal widening,
   callable assignability and runtime gradual checks.
5. Exact display size labels, depth counting, output-cap behavior, `help` topic
   lookup, and `show` disclosure defaults/budget application.
6. Precise builtin arities and edge cases not settled in the interview; regex
   syntax, sort key evaluation, conversion, Unicode, and serialization choices.
7. Exact fuel rounding/native-work charges and byte-unit interpretation.

Two interview tensions remain explicit: the approved short description lists
unary `!` among operators needing surrounding spaces while prefix forms are
allowed, and “missing keys crash” coexists with optional reads/narrowing. The
detailed rules above explain these boundaries, preserve the approved description
verbatim, and mark the proposed optional-read resolution. Fuel-based termination
is decided; it does not bound waiting for external tools.

The repository's existing stub also has only `run` and imposes deadlines. That
is an implementation gap, not authority to change this specification. Parser,
type-derivation, interpreter, and proxy-integration work must implement this target
before the README's stub behavior can be replaced.
