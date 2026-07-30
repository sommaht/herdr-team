# Errors — the four-layer ladder

Five gate rules touch errors (RS-001..005); this file is how the pieces fit. An error's
design question is always *which layer am I on* — each layer has its own naming,
composition, and audience.

> Worked examples are snapshots from real code — they demonstrate
> the style and may have drifted from that repo, which is fine. The doc is
> self-contained; don't go looking for these types in your own workspace.

## The ladder

**1. Leaf errors — a predicate or parse failed.** Condition-named (`InvalidPaneId`,
`NameUnavailable`) or operation-named (`ParseAgentInfoError`), whichever reads better —
specific either way, carrying the offending input or reason. In a bin crate a small
enum whose variants are the documentation is fine; a published library prefers std's
convention — an opaque struct with private fields (`ParseIntError`), reaching for
`#[non_exhaustive]` only where its semantics are wanted. One error per type is the
default, not a law (RS-005).

**2. Operation errors — one fallible operation's ways to fail.** Named for the
operation (`LoadContextError`, `FrameError` for framing), variants exactly its failure
modes, typed sources via `#[from]`/`#[source]`. A precondition check gets its own
standalone error — never a ride on a shared aggregate whose other variants that path
cannot produce (RS-005's honesty clause). Variant shape is per-variant honest (RS-004):
struct-with-source where call-site context earns fields (`Connect { path, source }`),
a bare `#[from]` tuple where it doesn't.

**3. Boundary aggregates — where paths genuinely fan in.** A command's or subsystem's
top-level error (`LaunchError`, `cmux::Error`) composes the operation errors below it,
usually via transparent or `#[from]` variants. This is the *only* layer where a large
enum is the right shape. Two things belong here and nowhere else:

- **Semantic distinctions the caller acts on** — recoverable vs terminal vs
  execution-ambiguous ("did the counterparty act before the connection died?") are
  documented properties of variants, not string content.
- **Repeated context-adding conversions** — when several call sites wrap the same
  source with the same context shape, `thiserror_ext::ContextInto`'s generated
  `.into_<variant>()` methods replace the hand-written `map_err` family (RS-001's cure
  at the aggregate layer; the crate is sanctioned).

**4. Presentation — one mapping, at the outermost boundary.** Typed errors stay typed
until the boundary that renders them (RS-003). A CLI maps each error type to a stable
exit code once, in an exhaustive match beside the enum (an `AsExitStatus` impl), so a
new variant forces an explicit decision; the render is one stderr line. Error text
**prescribes the fix first**, workaround second ("rename one of the agents…" before
"or address by pane id").

## Cross-cutting rules

- **Never stringify between layers** (RS-003): `Result<_, String>` past a boundary, or
  `.to_string()` while wrapping, destroys the chain the ladder exists to carry.
- **The chain is the diagnostic** (RS-002/RS-004): every wrapper exposes its source;
  discarding one requires strictly-more-data plus a site comment.
- **`map_err` adds context, `From` moves between layers** (RS-001): a repeated
  identical closure is a missing `From` impl or a `ContextInto` derive.
- **Don't pre-aggregate.** Reaching for one crate-wide `Error` enum first inverts the
  ladder — leaf and operation errors exist so that signatures say what can actually
  fail; the aggregate is earned at the fan-in, not assumed at the start.
