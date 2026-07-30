# Rust style gate — rule definitions

Rules with stable IDs, in three honest classes. **`script`** — a violation: the checker
proves the predicate mechanically. **`script-candidate`** — the checker surfaces the
shape, but the rule carries a justified-exception branch (a site comment, a doc
justification, an in-progress phase) that only the reviewer can verify; the walk
resolves each candidate to violation or cleared. **`judgment`** — requires semantic or
repository context; walked by hand. Report all three as
`RS-### file:line — one sentence`.

## Contents

- Errors (RS-001..005)
- Construction and conversion (RS-010..014)
- Serde and wire shapes (RS-020..022)
- Placement (RS-030..037)
- Data shape (RS-040..042)
- Abstraction and reuse (RS-050..054)
- Hygiene (RS-060..065)

Unassigned IDs inside these ranges are recorded in the comment at the end of this file.

## Errors

**RS-001 (script) — Repeated identical `map_err` closure.** The same `.map_err(...)`
closure appearing 2+ times in a crate is a missing `From` impl. One impl deletes every
occurrence; each call site becomes `?`.
```rust
// finding: .map_err(|source| Error::Driver(DriverFailure::new(source)))  x192
// fix:
impl From<rusqlite::Error> for Error {
    fn from(source: rusqlite::Error) -> Self { Error::Driver(DriverFailure::new(source)) }
}
```
`map_err` is legitimate when it adds call-site context a `From` impl cannot know
(path, line number, key name).

**RS-002 (script-candidate) — Source-discarding `map_err(|_| ...)`.** Dropping the underlying
error loses the diagnostic chain. Allowed only when the replacement captures strictly
more data (e.g. the offending input string) — and then a one-line comment at the site
says so.

**RS-003 (script) — Stringified typed errors.** `Result<_, String>` past a function
boundary, or `.to_string()`/`format!` used to wrap a typed error. Typed errors stay
typed until the boundary that maps them to user output.

**RS-004 (judgment) — Wrapper variant without `#[source]`/`#[from]`.** An error variant
that contains another error must expose it through `source()`. Variant *shape* is
chosen per variant, honestly: struct-with-source where call-site context earns fields
(`Connect { path, source }`), a bare `#[from]`/`#[source]` tuple where it doesn't
(`SetReadTimeout(#[source] io::Error)`). Mixing shapes within one enum is fine when
each variant's shape is its honest one — forcing uniformity would discard context or
mint meaningless field names.

**RS-005 (judgment) — Specific errors over aggregates; variants honest about their paths.**
Name an error for what went wrong — the operation (`LoadContextError`,
`ParseAgentInfoError`) or the condition (`InvalidPaneId`, `NameUnavailable`), whichever
reads better — never the bare subject (`ContextError`, `NameError`). A precondition check
gets its own standalone error type; making it ride a shared aggregate whose other variants
that path cannot produce is the finding. Reserve aggregate enums for boundaries that
genuinely fan in (a command's top-level error). The full four-layer ladder: [errors.md](errors.md).
```rust
// finding: ensure_name_available() -> Result<(), ContextError>  // drags in InvocationDir, NotInTmux…
// fix: the check owns its error; each command's enum carries only failures it can reach
pub struct NameUnavailable { pub name: AgentName, pub holder: PaneId }
```

## Construction and conversion

**RS-010 (script-candidate) — `fn try_new` where a std trait fits.** Single-argument validating
or converting construction is `TryFrom<T>` (or `FromStr` for strings). `try_new` is
acceptable only for multi-argument construction, stated in its doc comment.
```rust
// finding:
pub fn try_new(value: u16) -> Result<Self, PortError> { ... }
// fix:
impl TryFrom<u16> for Port { type Error = PortError; ... }
impl FromStr for Port { type Err = PortError; ... } // parse path delegates to TryFrom
```

**RS-011 (judgment) — Constructor honesty.** `new` is infallible; `From` for true
conversions; `with_*` for preconfigured variants. A `new` that can fail or a `from_*`
that panics is misnamed. A constructor documented as clamping/normalizing must not
panic on any input in its domain.

**RS-012 (script-candidate, report-only) — `as` integer casts.** Use `From`/`TryFrom` for
integer conversions. Exempt: deliberate numeric narrowing in DSP/indexing hot paths —
a domain layer may waive this by ID.

**RS-013 (judgment) — `FromStr`/`Display` are canonical text, not a codec.** These traits
belong on a type whose string *is* the value or its one canonical, human-writable
rendering (`Name`, `PaneId`, an enum token). A record whose string form is a
serialization format (e.g. JSON stashed in a tmux option) carries serde derives
describing its shape; the encode/decode belongs to the transport that owns the medium,
not to the record as `FromStr`/`parse`. This bounds RS-010 — it names conversions, not
codecs. Full pattern: [newtypes.md](newtypes.md).

**RS-014 (judgment) — Validated-string newtypes carry the standard view traits.** A
newtype that is "String + validation" derives `Deref` and `AsRef` (both forwarded to
`str`) and `Display`, plus `From<&T> for Cow<'_, str>` where builder APIs want it — and
no inherent `as_str()`. Without `DerefMut` every escape yields a fresh `&str`/`String`,
so construction-time validation can never be invalidated. `Borrow<str>` only when the
string form is canonical — exactly one valid spelling per logical value — because
`Borrow` promises `Eq`/`Hash` agreement forever (a pane id qualifies; a name that might
ever case-fold does not, and the exclusion is documented at the type). The test for
`Deref` is **behavioral equivalence**, not validation alone: the type qualifies while
its `Eq`/`Hash`/ordering and textual meaning remain those of its canonical `str` —
once constructed, it behaves exactly like a string. Construction-time canonicalization
that leaves comparison semantics identical to the *stored* form still qualifies; what
disqualifies is behavior-changing semantics — case-folded comparison, redaction, any
`Eq`/`Hash` that diverges from the stored `str`'s. Those take `AsRef` only, with the
opt-out documented at the type. Absence of `Deref` is a finding against this default, never
against a documented per-type choice. Getting these conceptually right up front is
what prevents hacky workarounds downstream. Complete recipe including derive
attributes and gotchas: [string-newtypes.md](string-newtypes.md).

## Serde and wire shapes

**RS-020 (script-candidate) — Hand-rolled `impl Serialize`/`Deserialize` without a stated gap.**
Requires an adjacent one-line comment naming what serde attributes cannot express
(e.g. a constant literal field beside `#[serde(flatten)]`). No comment = finding.

**RS-021 (judgment) — Manual wire-map building.** Functions that hand-assemble
`serde_json::Map`/`Value` for a struct's wire shape duplicate what derives express.
Derive with attributes; keep one source of truth for wire tokens. At crate scale this
becomes the API-crate pattern: [api-crates.md](api-crates.md).

**RS-022 (judgment) — serde needs are met with serde machinery, not hand glue.** A type
with `FromStr`/`Display` that needs serde uses
`serde_with::{DeserializeFromStr, SerializeDisplay}` or `#[serde(try_from = "…")]`.
Findings: a hand-written `TryFrom` impl that only forwards to `parse()`, and
`#[serde(transparent)]` on a type with a validating `FromStr` — that silently drops
validation on read. "The crate isn't a dependency yet" is not a reason: the sanctioned
set — `serde`, `serde_json`, `serde_with`, `thiserror`, `thiserror-ext`, `derive_more`,
`derive_setters`, `getter-methods`, `strum`, `itertools` — may always be added. The
escalation order (attributes → serde_with → try_from → deserialize_with → hand-rolled)
is in [string-newtypes.md](string-newtypes.md).

## Placement

**RS-030 (script) — Free function with an obvious owner.** A free function whose first
parameter is a same-crate type that has an `impl` block belongs on that type. A module
where some operations on a type are methods and others are free functions is the
canonical smell. Orphan-rule blocked → `*Ext` trait or relocate.

**RS-031 (script-candidate) — Crowded function.** Long is
allowed only when it reads as ordered named steps and splitting would hide the
transaction/state machine under review. Mixing wire/SQL/math detail with branching
policy is the finding, not length itself.

**RS-032 (judgment) — Single-consumer file split.** Don't mint a file for a handful of
lines or a single consumer. Splitting has a floor; a "role" (state/render/geometry/…)
is not automatically a file. A *substantial* single-consumer module is fine — it nests
under its consumer per RS-037; this rule targets minting tiny files, not nesting.

**RS-033 (script-candidate) — File size prompt.** Non-test file >400 lines: look for a real
domain boundary. Not a command to split mechanically — report as a prompt.

**RS-034 (script) — Duplicate function bodies.** Two functions with identical
normalized bodies (across files or within one) must be lifted to one owner. Copying a
sibling component's handlers is how 7-way duplication ships — which is the shape it is
aimed at. Only substantial bodies count, several statements deep: a repeated statement or
two is an idiom, or a delegation to the one owner this rule already asks for. The floor is
statements rather than characters, so the finding cannot be answered by rewrapping a line.

**RS-035 (judgment) — Published file layout.** A parent exposing bare `pub mod` lists
with zero curated `pub use` makes callers depend on file layout. Deliberate namespacing
(e.g. an API crate's wire-message modules, [api-crates.md](api-crates.md)) is fine when
it's a stated design; accidental exposure is the finding.

**RS-036 (judgment) — Constants and foreign-type functions have owners too.** RS-030's
logic extends past same-crate functions: a module-level constant whose name embeds a
type's name (`PANE_ID_FORMAT`) is an associated const on that type (`PaneId::FORMAT`);
a free function whose primary parameter is a foreign type becomes an `*Ext`-trait
method (`TmuxExt::execute`) **when method syntax or shared policy earns it** — a
one-off helper with no policy may stay local, per ext-traits.md's when-not-to list.
Surfacing the shape is a candidate, not an automatic conversion. Keep each thing with
the domain type it belongs to. When and how to reach for `*Ext`:
[ext-traits.md](ext-traits.md).

**RS-037 (judgment) — Single-consumer modules nest under their consumer; facades are
curated, not lint-driven.** A module with exactly one consumer lives under it
(`envelope` under `msg`, `placement` under `launch`), not as a sibling. A facade module
(`core`) re-exports its whole public surface and keeps submodules private; a dead-code
lint is never a reason to drop a facade re-export — wire a consumer or suppress with a
stated reason.

## Data shape

**RS-040 (script-candidate) — Tuple across a boundary.** Tuple type aliases and 3+-tuples
crossing function boundaries become named structs. Adjacent same-typed tuple fields
swap silently under refactor — the type looks safe and isn't. Exempt: heterogeneous
behavior-chain tuples implementing a composition trait (`(A, B, C)` as a processing
chain, [composition.md](composition.md)) — those are compile-time composition, not
data crossing a boundary.

**RS-041 (judgment) — Stringly-typed selector.** A `&str`/`String` parameter compared
against known literals is an enum. Parse once at the boundary (`FromStr`), match
exhaustively after.

**RS-042 (judgment) — Parameter pile.** More than ~3 parameters is a `*Params` struct;
struct literals are the named-argument syntax. Bools do not select modes.

## Abstraction and reuse

**RS-050 (judgment) — Unjustified abstraction.** Every new type/trait/helper/module
carries a one-line justification (doc comment). Can't state it in one line → don't
introduce it. This rule binds the agent's unprompted output only — never cite it
against something the user asked for.

**RS-051 (judgment) — Single-implementer trait.** A trait with one impl and no named,
planned second implementer is premature; use the concrete type until the second
implementer exists or is imminent and named. **Exempt: `*Ext` extension traits** — one
blanket impl is their finished form, not a missing implementer; see
[ext-traits.md](ext-traits.md). The tell is **use**, not impl target: an Ext trait adds
call syntax and policy and is never consumed as a polymorphic contract — nothing takes
`T: FooExt` bounds awaiting other implementors — it earns its keep through method
syntax and/or a policy home. The exemption cuts both ways: a speculative interface
does not escape this rule by adopting the `Ext` suffix.

**RS-052 (judgment) — Reuse before reinvention, both altitudes.** Before a new
function/algorithm/subsystem: name what was checked at symbol level (crate surface,
std, dependencies) AND algorithm level (does this computation already exist — in this
crate, a dependency, or `references/`?). The review answer must name files. "Nothing
found" without named files checked = finding.

**RS-054 (judgment) — Reuse reaches down to expressions, and the user's edits set
idiom.** Before hand-rolling a conversion expression, check the value's type for the
impl that already does it — `TmuxOutput: Display` turns
`String::from_utf8_lossy(&output.stdout()).into_owned()` into `output.to_string()`.
When the user hand-edits a pattern at one site, sweep the identical sibling sites in
the same pass, or name them. Prefer the crate's own domain types at call sites over raw
strings/options threaded around them.

## Hygiene

**RS-060 (script) — `unwrap()` in non-test code.** Use `?`, `expect("invariant: ...")`
with a proof obligation, or a typed error. Tests may unwrap freely.

**RS-061 (script-candidate) — Swallowed Result.** `let _ = fallible_external_op(...)` hides
partial failure. Handle it, propagate it, or state why ignoring is correct at the site.

**RS-062 (script-candidate) — `#[allow(dead_code)]`.** Only for an in-progress module, removed
when wired in. An allow that survives a phase is deletion deferred.

**RS-063 (judgment) — Commented-out code.** Delete it; git remembers. Dead match arms
and "maybe later" blocks are findings.

**RS-065 (judgment) — Tests that test the dependency.** A test asserting a derive's or
dependency's own behavior (a derived getter returns what the setter set) is a finding;
so is re-covering a rule already tested at a more meaningful consumer level. Test each
domain rule once, at the highest level that owns it.

<!-- Unassigned IDs, triaged 2026-07-24: RS-053 (1:1 dependency-wrapper module as a
per-se finding) was DECLINED by Alec — the tmux.rs seam had some value; the real defect
was under-use of the crate's own domain types, a domain judgment not encodable as a
shape rule (its salvageable half lives in RS-054's last sentence). Do not re-propose.
RS-006 (error messages prescribe the fix first) is resolved: its substance is recorded
as house guidance in errors.md's presentation layer, and the ID stays unassigned
unless finding-status is ever wanted. RS-023 (wire tokens cite counterparty
provenance) and RS-064 (bare #[must_use] without a reason string) remain untriaged —
re-raise only if fresh evidence accrues. -->
