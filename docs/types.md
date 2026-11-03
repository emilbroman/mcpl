# Schema-derived types

`mcpl::types` implements discovery-time type derivation and rendering from
[language specification sections 7–8](language.md#7-values-and-types). It does not
execute MCPL, check program assignability, or replace JSON Schema validation.
Raw schemas remain on `registry::ToolDefinition` for the call boundary.

```rust
use mcpl::types::{derive_schema, Type};
use serde_json::json;

let derived = derive_schema(&json!({
    "type": "object",
    "properties": {"url": {"type": "string"}},
    "required": ["url"],
    "additionalProperties": false
}));
assert_eq!(derived.ty.to_string(), "{url: string}");
assert!(derived.notes.is_empty());
```

## API

- `derive_schema(&Value) -> DerivedType`: an infallible approximation with `ty`,
  `definitions` (named recursive types), and `notes` (JSON Pointer + explanation).
- `derive_tool(server, &ToolDefinition) -> DerivedType`: a tool capability with
  one `args` parameter. An output schema supplies the result type; without one,
  the result is `Content[]` and its five-variant definition is included.
- `derive_server(server, tools) -> DerivedType`: a first-class server type with
  the original tool names, in the iterator's order. Pass a registry snapshot's
  `tools.values()` for its stable sorted order.
- `Type` and `FunctionType` implement `Display`. Tool values display their
  function signature; server values display the record of tool signatures.
  `render_tool_signature(&ToolType)` adds the qualified tool name for help.
- `DerivedType::render_definitions()` prints each named definition once, sorted
  by name, for full help. Keep definitions alongside the type when storing it.
- `content_type()` exposes the full discriminated union, with auxiliary content
  records inlined. `content_result_type()` returns the short `Content[]` form.
- `union()` flattens unions, removes duplicate alternatives in encounter order,
  and simplifies unions containing `any`. Use it instead of constructing raw
  `Type::Union` when normalization is needed.

`Type::Number { integer }` is **one numeric type**. The flag is display metadata,
not an assignability restriction. Derived `PartialEq` is representation equality
(including metadata and capability identity), not the language's assignability
or runtime equality relation. The checker must treat both numeric flag values
as numbers. Likewise, record optionality and tuple optionality are distinct:
optional record reads add `null`; absent tuple positions fail at runtime.

## Deterministic approximation choices

- Property/enum order follows the original JSON order (`serde_json`'s
  `preserve_order` feature). Named fields precede index signatures. Unsafe bare
  names are JSON-quoted, not rewritten.
- Prefix tuple positions are optional unless required by `minItems`; open tails
  display as `...T[]`. `items: false` closes the tail. Legacy array-form `items`
  uses `additionalItems`.
- Records are open by default. Pattern property values join permitted additional
  values in the dynamic signature. Named fields keep their independent types.
- Without `type`, structural keywords alone do not assert object/array types:
  the unconstrained non-object/non-array cases are conservatively kept as `any`.
- Compatible intersections merge record fields, requiredness, index signatures,
  arrays and scalar/literal unions. Unrepresentable intersections fall back to
  `any` with a note; there is no public bottom or intersection type.
- `not` preserves an independently known base. Conditionals union `then`/`else`
  (an absent branch is unconstrained), then intersect with the base. Validation
  constraints and annotations do not add types or insert defaults.
- Local JSON Pointers (including escaped/percent-encoded fragments), `$anchor`,
  and schema resources identified by `$id` resolve without I/O. Older
  `definitions` and draft-04 `id` work; draft-04/06/07 `$ref` siblings are ignored.
  Recursive references become names, preferentially using the definition key.
  Collisions get deterministic numeric suffixes; merged tool/server scopes
  rename references and definitions together. `Content` is reserved for the
  fixed MCP union (`SchemaContent` is used for a recursive schema of that name).
- Dynamic/recursive-reference keywords, external references, unknown extensions,
  and applicator-sensitive unevaluated constraints retain independently known
  information and record notes. No network/file schema retrieval occurs.
- A schema size/depth preflight and reference expansion budget bound derivation
  work. Oversized schemas become `any` with a note; expansion limits can preserve
  surrounding structure. These discovery limits are separate from runtime fuel.

## Regression data

The [real Playwright fixture](../tests/fixtures/README.md) captures all 25 tools
from `@playwright/mcp@0.0.84`. Every tool derives without approximation/fallback
notes; no input field becomes `any`. All tools return `Content[]` because none
advertises `outputSchema`. Exact live signatures include:

```text
playwright.browser_navigate: (args: {url: string}) => Content[]
playwright.browser_snapshot: (args: {target?: string, filename?: string, depth?: number, boxes?: boolean}) => Content[]
```

The specification's two-tool example is illustrative: its empty snapshot input
is not the real package's current schema. Tests cover both examples and the live
fixture, plus every keyword-table family, malformed schemas, recursive naming,
and bounded expansion. Run `cargo test --locked` without Node or network access;
only recapturing the fixture needs npm.
