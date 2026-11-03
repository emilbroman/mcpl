# Runtime and session integration

The stdio binary exposes `run`, `help`, and `show`. The exact descriptions and
schemas are tested against [the language specification](language.md). All
language results are one MCP text block; failures set `isError: true`. Unknown
MCP tool names remain protocol errors. Downstream tools are not advertised
individually.

## Evaluation and commits

`ProxyServer` serializes requests against its connection-local `SessionState`.
The complete program is checked before evaluation. Each successful statement
commits the frontend's complete planned binding and automatic-name counter;
failure or cancellation leaves earlier commits intact. Truncated output never
deletes committed bindings. `show` runs the same checker/evaluator with fresh
budgets, but neither commits nor allocates a name. It may perform tool effects.

`Evaluator` runs an explicit continuation stack, including collection callbacks.
Calling an `any` function recursively does not recurse on the Rust stack.
Callbacks and downstream calls run sequentially. Periodic cooperative yields
allow cancellation and other sessions to progress; yields and waiting do not
consume fuel. There are no connection, discovery, call, evaluation, or shutdown
deadlines. A downstream that never responds must be explicitly cancelled.

Runtime values are immutable, shared allocations. The evaluator uses the
checker's resolved roots/member keys rather than reinterpreting dotted names.
Closures retain captured values and their original checked code. Capturing a
global counts as a lookup; subsequent use of the capture does not refresh that
global's age. A stored closure failing in a later request reports the current
call expression and identifies the stored closure, rather than displaying an
old byte span against unrelated source.

All 52 builtins are implemented. Regex syntax is Rust `regex`, with Unicode
support, no look-around/backreferences, and literal replacement strings.
Unicode case and whitespace behavior follows the Rust standard library and the
locked regex dependency. Numeric values are finite f64; string indexing and
length use Unicode scalar positions. `type_of` preserves an available checked
type and uses runtime shape for gradual `any` data; capabilities disclose only
their signatures.

## Resource accounting

The compiled defaults are 10,000,000 fuel units, 50 downstream calls, 64 KiB
source, 100,000 entries per array/object, 16 MiB strings, 64 container levels,
256 expression levels, and 256 MiB logical session memory. The frontend's
additional static expansion budgets still apply. `show` gets the same execution
budgets as `run`.

Fuel charges compose: expression visits, collection construction/copying,
string/JSON scans, runtime type checks, callback evaluation, equality,
serialization, regex work, and sorting. Sorting charges
`n * ceil(log2(max(n, 1)))` in addition to evaluating each key once. Native
scans use deterministic, conservative charges rather than elapsed CPU time.
A sent downstream call costs 10 units and one call slot; invalid arguments
rejected before sending do not consume a slot. Limits are errors, while output
limits are disclosure truncation.

Memory is logical retained storage, not an allocator/RSS promise. Accounting
includes values, container slots/keys, runtime type data, closure bodies and
captures, retained checked code, and permanent binding/type/name metadata.
Shared value and code allocations are counted once when pressure requires an
exact graph walk. A conservative expanded-size estimate provides the fast path.
LRU eviction removes access through a binding name, not values retained by
aliases, closures, or active evaluation. Typed tombstones remain reserved and
budgeted. If eviction cannot make room, the active statement fails without a
commit. The Rust embedding API can lower the memory ceiling for tests; MCP
clients cannot change it.

Active roots and continuation storage are checked at deterministic checkpoints
(every 1,024 continuations or 1 MiB of tracked new allocations), and again before
returning a result. Individual native operations precheck their growth and
charge their traversals. Peak physical memory can exceed logical storage during
native work, SDK decoding, and static checking. Recursive capture chains are
also bounded to 64 and runtime type construction has a separate expansion cap
to protect host stacks and metadata allocation. These additional failures are
reported as resource limits, not hangs or host recursion failures.

The MCP SDK decodes messages before the evaluator receives values. These
in-process limits are not an OS sandbox or a hardened transport-level quota for
hostile downstream processes; configuration and downstream executables must be
trusted. Downstreams' own CPU, memory, and side effects are outside MCPL's budget.

Downstream stdio messages have a 128 MiB wire-line cap, sufficient for a maximally
JSON-escaped 16 MiB string plus its envelope. Exceeding that cap closes the
connection instead of silently dropping a response. Normal stdio shutdown closes
stdin and waits for the child without a deadline; abandoning the transport kills
and reaps the child. Streamable HTTP retains the SDK's 16 MiB SSE-event cap, so
large HTTP results may hit that transport limit before language value validation.

## Results, discovery, and disclosure

Arguments and schema-backed results are validated against raw JSON Schema,
including supported format assertions. With `outputSchema`, present
`structuredContent` is authoritative; otherwise exactly one text block may be
parsed as JSON and validated. Without `outputSchema`, the value is always the
`content` array, even if structured content also exists. Tool-reported errors
are checked first and retain the tool's text. No tool retry is automatic.

`help` uses discovered signatures and descriptions. Builtin documentation comes
from the checked-in normative standard-library tables. Recursive definitions
are printed once when relevant; the fixed `Content` definition is not expanded
in every tool signature. Exact builtin/server topics win, then the longest
configured server prefix identifies a tool topic.

Binding previews are balanced, at most 600 Unicode scalars and 2,400 bytes,
with depth 2, five array items, eight object keys, and 120 string scalars.
Run output is at most 8 KiB, reserving space for a failure diagnostic and an
omitted-binding count. Help/show have 64 KiB ceilings; show can request less.
Omitted-item counts remain exact. Rendering stops at its budget and never
serializes an entire large value merely to truncate the resulting string.

## Verification

`cargo test --locked --all-targets` covers all worked examples against fake MCP
servers, every builtin family, progressive disclosure, cross-run closures and
bindings, schema result selection, preflight rejection, partial commits, LRU,
shared allocations/tombstones, cancellation propagation, and resource failures.
Virtual-clock tests prove discovery/calls do not acquire deadlines. The Pi test
rig additionally exercises real stdio MCPL with a fake model; a live model run
is a manual smoke check, not part of the automated test suite.
