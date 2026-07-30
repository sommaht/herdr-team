# Newtypes — the general pattern

A newtype does two jobs, and both matter:

- **Proof-carrying**: construction is the only gate, so every holder downstream is
  valid by construction.
- **Distinctness**: same-shaped values stop being interchangeable. Two bare `String`s
  swap silently — the wrong field compiles, ships, and fails at runtime. With newtypes
  the compiler refuses the swap: an `AgentName` cannot land in a `model:` field, and
  `PaneId`/`WindowId`/`SurfaceId` cannot be crossed. Misuse doesn't become impossible —
  it becomes **explicit**: you'd have to extract the inner value and construct the
  other type (`Model(name.to_string())`), an act that's visible in a diff, where a
  bare-string swap is invisible. This is RS-040's tuple argument at field scale, and it
  holds for *every* stance below — distinctness is the floor value; validation is what
  varies. It shows plainest in unit newtypes, but id families earn it just as hard.

Under-use of domain newtypes is how raw strings and threaded `Option<&str>` hacks creep
into call sites — reach for the domain type first. String-backed newtypes have their
own recipe on top of this: [string-newtypes.md](string-newtypes.md).

> Worked examples are snapshots from real code —
> they demonstrate the style and may have drifted from those repos, which is fine. The
> doc is self-contained; don't go looking for these types in your own workspace.

## Pick the stance first

Every newtype takes exactly one **validation stance**, and the whole API follows from
it. Mixing stances is the characteristic bug (e.g. `#[serde(transparent)]` — the
trusted-carrier treatment — on a validated type silently drops its validation on read).

| Stance | Meaning | Construction | serde | Exemplar |
|---|---|---|---|---|
| **Validated refinement** | "any inner value that passed my predicate" | `FromStr`/`TryFrom`, fallible | re-validate on read (`serde_with`) | `NonEmptyString`, `Normal`, `PaneId` |
| **Trusted carrier** | "I carry the owner's value verbatim; the counterparty rejects by name" | infallible; `From<String>`/`From<&str>` *only if* publicly mintable, else a scoped constructor | `#[serde(transparent)]` | protocol strings (`SurfaceId`) |
| **Mere distinctness** | "same shape, not interchangeable — wrong-field bugs become compile errors" | infallible; same authority caveat | `#[serde(transparent)]` | `Model`, id families, `Hertz` (a *unit* — see below) |

```rust
pub struct NonEmptyString(String); // refinement:   FromStr rejects blank input
pub struct Normal(f32);            // refinement:   TryFrom rejects values outside 0.0..=1.0
pub struct SurfaceId(String);      // carrier:      From<String>, transparent — no opinion
pub struct Model(String);          // distinctness: From, transparent — just not a bare String
pub struct Hertz(f32);             // distinctness + curated arithmetic — a unit newtype (below)
```

(The mere-distinctness row means *no additional* validation or transport policy — the
distinctness value itself is the floor shared by every row. And the table is the
**validation/transport stance** only; *who may construct* is a second, orthogonal
decision — see Construction authority below.)

Refinement types come in two flavors. **Pre-validated primitives** carry their
predicate in the name (`NonEmptyString`, `Normal`) — reusable vocabulary any domain can
consume. **Domain-role types** carry a predicate that is domain policy (`PaneId`
accepts only `%N`). Prefer the predicate-named primitives as building blocks; a
role-named type earns its place when the rule *is* the domain's own.

The stance decides three things at once — keep them consistent:

1. **Construction honesty** (RS-010/RS-011): fallible single-arg construction is
   `TryFrom`/`FromStr`, never `try_new`; infallible is `From`. A trusted carrier that
   validates, or a refinement type with an infallible `From<String>`, is lying about
   its stance.
2. **serde treatment** (RS-022): a refinement type's wire boundary re-runs the
   predicate; a carrier's does not. Changing the serde attribute changes the stance —
   never do it to satisfy a lint or dodge boilerplate.
3. **`Borrow`/`Hash` eligibility**: only a type whose inner form is canonical (one
   valid spelling per logical value) may promise `Borrow` — details in
   [string-newtypes.md](string-newtypes.md).

## Construction authority — the orthogonal decision

The stance says how values are *checked*; authority says **who may mint one**. Decide
both, every time. A trusted carrier is not automatically publicly constructible:
`RequestId` is transparent on the wire yet deliberately constructible only inside its
crate — correlation ids are the connection's to mint, nobody else's. A public
`From<String>` on a carrier is a *choice*, right for values the caller legitimately
owns; a private field with a `pub(crate)` constructor is the same stance with a
narrower mint. **A `From` impl *is* public minting authority** — derive it only when
that is the intent. State the authority in the type's doc line, and don't let a
recipe's derive block decide it by default.

## Unit newtypes (quantities)

`Copy` numeric wrappers whose point is dimensional meaning — `Hertz`, `Decibel`,
`GainFactor`, `SampleRate`. Usually the distinctness stance (infallible `From`),
occasionally refinement when a range is enforced (`Normal`, confined to `0.0..=1.0`). What makes them their own kind is
that the API *is* the arithmetic surface, curated per dimension:

- **Implement the ops the dimension allows** — `Hertz + Hertz`, `Hertz * f32` — and
  **deliberately withhold the ones it doesn't**: no `Hertz * Hertz`, no
  `Decibel + GainFactor` (that's a conversion, not an addition). The withheld ops are
  the design; they're what the raw `f32` couldn't refuse.

  ```rust
  #[derive(Copy, Clone, Debug, PartialEq,
           derive_more::From, derive_more::Add, derive_more::Sub, derive_more::Display)]
  pub struct Hertz(f32);
  // Hertz + Hertz ✓   Hertz - Hertz ✓   scalar Mul<f32> ✓ (one impl or derive)
  // deliberately absent: Hertz * Hertz — the op the dimension forbids
  ```
- **Mechanism: `derive_more`'s conversion/arithmetic/display derives** (`From`, `Into`,
  `Add`, `Sub`, `Mul`, `Neg`, `Display`) — it's in the sanctioned set. A hand-rolled
  declarative macro layer (`impl_from!`-style) is legacy: keep one only for a shape
  `derive_more` cannot express, stated in its doc comment.
- **Escape is explicit, not ambient.** Unlike string newtypes, these do *not* take
  `Deref` — dereferencing to the raw number would put the undimensioned value back into
  every arithmetic expression, which is exactly what the type exists to prevent. The
  raw value exits through a named accessor or a `From` conversion at the boundary that
  needs it, with the conversion math visible (`From<Decibel> for GainFactor`).

## What the type owns

- **Its validation** — the predicate lives in the constructor path, nowhere else. No
  call-site re-checks, no validation inside a codec (serialization and validation are
  distinct jobs; the wire boundary *invokes* the predicate via the construction path,
  it doesn't own a copy).
- **Its related constants** (RS-036) — formats, keys, sentinels: `PaneId::FORMAT`,
  `Agent::KEY`, not module-level `PANE_ID_FORMAT`.
- **Its rendering** — `Display` when there is a canonical rendering; error types
  interpolate the newtype directly rather than reaching through `.0`.
- **Not its transport.** A record that happens to travel as JSON does not get
  `FromStr`/`parse` for that encoding (RS-013); the vehicle that owns the medium does
  the encode/decode, and the type contributes only serde derives describing its shape.

## Access and escape

- Getters via the sanctioned derive crates (`getter-methods` maps `Option<T>` →
  `Option<&T>`; `derive_setters` for builder-style setters) rather than hand-written
  accessor blocks.
- View traits (`Deref`/`AsRef`/`Cow`) let a value escape as a borrow without an
  inherent `as_str()`-style method — the string-specific rules are in
  [string-newtypes.md](string-newtypes.md).
- Escaping is safe by construction: a borrow of the inner value can't mutate the
  newtype, so the predicate that ran at birth still holds for the value's lifetime.

## When not to mint one

- No domain meaning, one call site — the RS-050 one-line justification test applies to
  newtypes like everything else. "Wraps a String" is not a justification; "a validated
  agent name; parent is reserved" is.
- A closed set of known values is an **enum** (RS-041), not a validated string.
- Multi-field data crossing a boundary is a named struct (RS-040) — a newtype is
  single-field by definition; don't stretch it.
