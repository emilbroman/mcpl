# Parser and type checker

`mcpl::frontend` is the implemented, **preflight-only Rust library** for the
[MCPL language specification](language.md). It parses and checks programs against
session types, schema-derived servers, and builtins. It does not evaluate
expressions, call downstream tools, mutate the supplied environment, or commit
session bindings. The upstream MCP server integrates this library with the
runtime: `run` checks a whole program before executing it, and `show` checks one
expression before evaluation. There is no standalone frontend CLI.

## Check a program or expression

This example can be used as a Rust example binary in this package. No downstream
server needs to be running: the tool definition stands in for discovery metadata.

```rust
use mcpl::frontend::{Environment, check_expression, check_program, render_diagnostics};
use mcpl::registry::ToolDefinition;
use mcpl::types::derive_server;
use serde_json::json;

fn main() {
    let mut environment = Environment::default();
    assert_eq!(environment.builtins.len(), 52);

    let tools = [ToolDefinition {
        name: "lookup".into(),
        description: Some("Look up a document title".into()),
        input_schema: json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"],
            "additionalProperties": false
        }),
        output_schema: Some(json!({
            "type": "object",
            "properties": {"title": {"type": "string"}},
            "required": ["title"],
            "additionalProperties": false
        })),
        annotations: None,
    }];
    environment
        .add_server("catalog", derive_server("catalog", &tools))
        .expect("valid, unreserved server name");

    let source = "title = catalog.lookup({query: \"MCPL\"}).title; len(title);";
    let checked = check_program(source, &environment)
        .unwrap_or_else(|errors| panic!("{}", render_diagnostics(&errors, source)));
    assert_eq!(checked.statements[0].name, "title");
    assert_eq!(checked.statements[0].binding.display_type.to_string(), "string");
    assert_eq!(checked.statements[1].name, "res1");
    assert_eq!(checked.statements[1].binding.display_type.to_string(), "number");
    assert_eq!(checked.statements[1].next_result, 2);

    let expression = "map([1, 2], (x) => x + 1)";
    let checked_expression = check_expression(expression, &environment)
        .unwrap_or_else(|errors| panic!("{}", render_diagnostics(&errors, expression)));
    assert_eq!(checked_expression.info.display_type.to_string(), "number[]");

    // Checking plans bindings, but neither executes the lookup nor stores title.
    assert!(!environment.bindings.contains_key("title"));
    assert_eq!(environment.next_result, 1);
}
```

### Public entry points

- `parse_program(&str) -> Result<Program, Diagnostic>` parses semicolon-terminated
  top-level statements, without name resolution or type checking.
- `parse_expression(&str) -> Result<Expr, Diagnostic>` parses one expression,
  without a top-level terminating semicolon. A parenthesized sequence is allowed.
- `check_program(&str, &Environment) -> Result<CheckedProgram, Vec<Diagnostic>>`
  parses and checks the whole program. Later statements see earlier planned
  bindings. Any error prevents a checked program from being returned.
- `check_expression(&str, &Environment) -> Result<CheckedExpression, Vec<Diagnostic>>`
  checks one expression without allocating an automatic name or session binding.
  The server uses this preflight entry point for `show`; checking alone does not
  evaluate the expression.

`Program`, `Expr`, and `Span` are re-exported at `mcpl::frontend`; statement and
expression variants live in `frontend::ast`. The checked result types are
re-exported too; detailed metadata types live in `frontend::checker`.

## Environment and session integration

`Environment::default()` supplies all 52 builtins, their reserved names, the fixed
`Content` definition, empty binding/server maps, and `next_result = 1`.
`Environment::bind(name, ty)` is a setup helper: it inserts a `Binding::new(ty)`
and reserves the name. It is not a checked declaration or a runtime commit API.

`Environment::add_server(name, derived)` accepts a `DerivedType` whose root is
`Type::Server`. Use `types::derive_server` with discovered `ToolDefinition`s;
[the type derivation API](types.md) retains original tool names and signatures.
Server names must be valid MCPL symbols and cannot collide with bindings,
builtins, servers, or reserved tombstones. Named definitions from each import are
merged collision-free: conflicting names receive deterministic numeric suffixes,
with all references renamed together. `Content` is the one fixed shared global
definition; a conflicting definition is rejected. Failed imports leave the
environment unchanged. `add_builtin` similarly rejects reserved-name collisions.

The environment fields are public integration data, not an enforced session
store. A caller maintaining them directly must preserve these invariants:

- `bindings` holds complete `Binding` values: precise `ty`, listing-oriented
  `display_type`, and optional builtin identity in `builtin`.
- `servers`, `builtins`, and `definitions` describe the static global environment.
  Keep named definitions available as long as stored types can refer to them.
- `reserved` retains every name ever committed, **including evicted bindings**.
  On eviction remove the runtime value, not its type binding or reserved name:
  retaining typed tombstones lets preflight succeed and leaves the specific
  "was evicted" diagnostic to the runtime.
- `next_result` is the next automatic-name candidate, not the number of bindings.

A `CheckedProgram` contains the original `program`, an ordered `statements`
plan, and an `expressions` metadata map. Each `CheckedStatement` contains the AST
`statement`, chosen `name`, complete `binding`, and the `next_result` value to
commit **only after that statement succeeds at runtime**. Explicit declarations
and generated `resN` names are checked for collisions, including declarations
later in the same program. Checking itself changes none of the supplied state.

The server preflights the entire program before executing any statement.
After each successful statement, `SessionState::commit` stores its value, inserts
the **complete planned `Binding`** into `environment.bindings`, adds its name to
`reserved`, and sets `environment.next_result` to the planned counter. Embedders
must preserve this contract: rebuilding with
`Binding::new(binding.display_type)` loses literal precision and builtin identity.
If a later statement fails, earlier successful commits remain; the failed
statement and remaining plans are not committed. The frontend itself still
performs none of these runtime operations.

## Checked expression metadata

Both checked result types expose `expressions: BTreeMap<Span, ExpressionInfo>`;
`CheckedExpression` also exposes the root `expression` and its `info` directly.
Spans are UTF-8 **byte offsets with an exclusive end**, not character columns.
Use the AST node's span to look up inferred metadata. An `ExpressionInfo` has:

- `ty`: precise inference data, retaining literals needed for assignability,
  discriminants, and contextual checking.
- `display_type`: widened/listing-oriented type. For example, the binding from
  `answer = 1;` retains literal `1` in `ty`, but displays `number`.
- `builtin`: optional direct builtin identity, preserved when aliasing.
- `path`: an optional static `ValuePath` (`Root` plus exact keys), used for
  bounded flow narrowing.
- `resolution`: optional `Resolution` describing static symbol/member lookup.
  Its root is a session binding, lexical local with a scope ID, server, or
  builtin. A member access on an already computed receiver can have no root.
  Each `MemberAccess` records the exact joined `key`, source `span`, inferred
  `ty`, and `optional` flag. The flag distinguishes declared optional fields,
  whose absent read supplies `null`, from required reads. Explicit index nodes
  retain their index AST; not every node has a `Resolution`.

The checker supports gradual `any`, structural assignability, contextual lambdas
and builtin calls, immutable scopes, and bounded null/discriminant narrowing.
It rejects unsupported coercions, non-boolean conditions, missing known fields,
and invalid tool arguments. Successful checking is not a promise of runtime
success: array bounds, actual schema validation, evicted values, tool failures,
and evaluation/resource failures still belong to execution.

### First-class builtin types

`types::Type::Builtin(BuiltinType { name, signature })` preserves a builtin's
polymorphic identity through aliases, arrays, records, and other containers.
This is distinct from an ordinary monomorphic `Type::Function`: a saved `map`
value must still infer its element/callback types at its later call site.
Contextual function use can specialize the builtin signature. `Display` renders
the signature rather than an internal identity marker. JSON Schema derivation
never emits `Type::Builtin`; it is a frontend capability type. There are still
52 builtins in the catalog; this representation adds no builtin operations.

## Parsing and static resolution

The parser implements the specification's PEG-equivalent syntax using a lexer,
Pratt expression parser, and recursive descent for structural forms; it is not a
generated PEG parser. `ExprKind::Group` preserves parentheses explicitly, so
parenthesized grouping is not erased when calculating spans or AST depth.
Sequences have their own `ExprKind::Sequence` with preceding local/expression
statements and a final value expression.

Top-level `name = expression;` declares a session binding, including a literally
dotted name such as `a.b`. Bracket-local sequences are admitted directly inside
parentheses, array elements, object values, and call arguments. They end in an
expression, not a trailing semicolon. Lambda bodies, ternary branches, and index
expressions need parentheses to contain a sequence. Locals and lambda parameters
may shadow any outer name, but cannot be redeclared in the same lexical scope;
locals do not produce session bindings or consume `resN` names.

Dotted names are statically resolved by **longest match**, not by splitting a
source string into runtime field accesses:

1. Search lexical scopes innermost-first, then global bindings/servers/builtins.
   In the first matching scope choose the longest leading symbol name.
2. At each statically known record/server step, choose the longest known joined
   member key. This is greedy, with no fallback to a shorter split on failure.
3. On `any`, consume one segment per key, regardless of runtime object contents.
4. String indexing, such as `s["x.y"]`, selects exactly one key without joining.

For example, with both `a` and `a.b` bound, `a.b.c` selects root `a.b` then key
`c`. With tools `x` and `x.y`, `s.x.y` selects the `x.y` tool. The runtime uses
these checked resolutions and index AST rather than independently re-splitting
source strings or guessing a split from runtime values. Hyphenated symbols are
also preserved: `a-b` is a name, while `a - b` is subtraction. Binary operators
require surrounding whitespace.

## Diagnostics and deterministic limits

`Diagnostic` has a `category`, `message`, byte `span`, and optional `hint`.
`Category` distinguishes `Syntax`, `Type`, `Runtime`, and `Limit`, displayed as
`syntax error`, `type error`, `runtime error`, and `limit exceeded`. Parser and
checker errors use the relevant frontend categories; the evaluator uses
`Runtime` and `Limit` for execution diagnostics through the same rendering API.

Use `diagnostic.render(source)` for one error or
`render_diagnostics(&diagnostics, source)` for the batch. Output includes a
one-based line/Unicode-scalar column, bounded source excerpt, caret underline,
and optional hint. Multiline spans show the first line plus a continuation
marker. Excerpts and diagnostic text are clipped to keep rendering bounded.
The parser returns its first error; checking collects up to ten distinct errors,
sorted by source position.

Frontend limits are independent of the evaluator's fuel and tool-call budgets:

- Source is limited to 64 KiB and expression/AST nesting to 256, including
  grouping and long operator/postfix chains, not just bracket nesting.
- Parsing and checking each run on a scoped worker with a bounded 16 MiB stack,
  so callers need not provide a large executor stack. Worker allocation failure
  produces a `Limit` diagnostic.
- Inferred type expansion has a 100,000-node budget. Type relations and recursive
  type/index traversal have deterministic work/depth budgets; exhausted relation
  proofs fail conservatively rather than accepting an unproved relation.

There are no wall-clock deadlines or tool I/O in the frontend. These static
bounds are separate from the implemented runtime's fuel, memory accounting,
LRU eviction, and downstream-call limits.

## Regression coverage

Run `cargo test --locked` from the repository root. `tests/frontend_check.rs`
covers syntax, resolution metadata, typing, builtin inference, and bounded
inputs. `tests/frontend_spec.rs` checks the worked programs and valid fenced
examples from `docs/language.md` using the source corpus under
`tests/fixtures/frontend/` without calling a downstream server.

All twelve diagnostic templates in specification section 13 have golden text.
Eight syntax/type cases actually execute the frontend checker. The remaining
four (array index, downstream failure, eviction, and fuel exhaustion) exercise
only diagnostic rendering with constructed diagnostics. Those frontend goldens
test formatting, not execution. Runtime and session tests separately cover
evaluation failures, binding persistence, partial commits, eviction, and resource
limits.
