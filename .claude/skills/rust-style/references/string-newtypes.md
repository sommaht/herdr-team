# String newtypes — the specific recipe

Builds on [newtypes.md](newtypes.md): pick the stance first. This file is the complete
treatment for string-backed newtypes — the most common kind, and the one with the most
trait-surface decisions. Getting these conceptually right up front is what prevents
hacky workarounds as code accrues.

> Worked examples are snapshots from real code — they demonstrate
> the style and may have drifted from that repo, which is fine. The doc is
> self-contained; don't go looking for these types in your own workspace.

## Validated refinement — the full derive block

Shape sketch, not paste-ready: imports (`Cow`, `FromStr`), the error type, and the
`from_str` body are elided.

```rust
use serde_with::{DeserializeFromStr, SerializeDisplay};

/// A string guaranteed non-blank: at least one non-whitespace character.
#[derive(Clone, Debug, PartialEq, Eq, Hash,
         SerializeDisplay, DeserializeFromStr,          // serde through FromStr/Display
         derive_more::Display, derive_more::AsRef, derive_more::Deref)]
#[as_ref(forward)]                                       // AsRef<str>, AsRef<[u8]>, …
#[deref(forward)]                                        // Target = str, NOT String
pub struct NonEmptyString(String);

impl FromStr for NonEmptyString { /* the predicate lives here, nowhere else */ }

impl<'a> From<&'a NonEmptyString> for Cow<'a, str> {     // for Into<Cow<'_, str>> builder APIs
    fn from(value: &'a NonEmptyString) -> Self { Cow::Borrowed(value) }
}
```

- **No inherent `as_str()`.** The view traits cover every consumer; `&value` coerces
  where `&str` is expected via `Deref`.
- **`#[as_ref(forward)]` is broad by design** — it forwards every `AsRef` the inner
  `String` offers (`str`, `[u8]`, `Path`, …). When the contract should be `AsRef<str>`
  only, write `#[as_ref(str)]` instead.
- **`#[deref(forward)]` matters** — it makes `Target = str`. Forwarding to `String`
  instead makes the `as_str()` deletion cosmetic rather than real.
- **`Deref` here is safe, not a smell**: with no `DerefMut`, no `str` method can obtain
  mutable access to the wrapped `String` — the newtype itself is untouchable, so
  construction-time validation holds for the value's whole lifetime. The worst outcome
  is holding a plain `&str`, which is exactly what `as_str()` handed out explicitly.
- **`Deref` is the default, not a mandate — the test is behavioral equivalence**
  (RS-014): the type qualifies while its `Eq`/`Hash`/ordering and textual meaning
  remain those of the stored canonical `str`. A refinement whose comparison semantics
  diverge — case-folded equality, redaction — takes the **`AsRef`-only branch**: drop
  `Deref` from the derive list, keep `AsRef`/`Display`, and document the opt-out at
  the type.
- **`#[into(Cow<'_, str>)]` gotcha**: naming types in derive_more's `#[into(...)]`
  *replaces* the default `From<Self> for String` — and the derive can only generate the
  consuming impl. The borrowed impl above is the one call sites want; write it by hand.

## `Borrow<str>` — only for canonical forms

`Borrow`'s contract is that the borrowed form hashes and compares identically to the
owned form, forever. The test: **is there exactly one valid spelling per logical
value?** A pane id qualifies. A name that might ever case-fold does not — normalizing
later silently breaks map lookups (entries become unfindable, nothing panics). Decide
at birth; document the exclusion at the type so the next reader (or LLM) doesn't
quietly add it alongside case-insensitivity.

## FromStr vs TryFrom, honestly

Conceptually these types are *refinement, not decoding* — `NonEmptyString("alice")` has
no separate value that `"alice"` encodes, which is a fair argument that `FromStr` (whose
home turf is `"127.0.0.1"` → four bytes) is a loose fit. Keep `FromStr` anyway, for the
ecosystem hooks that hard-require it — clap's derived parser, `MaybeStdin<T>` (which
hard-codes `T::from_str` internally), `serde_with::DeserializeFromStr`, `.parse()` —
and treat `Display` as its inverse. The alternatives exist but cost more:
`#[serde(try_from = "String")]` and clap's `ValueParserFactory` cover serde and clap
via `TryFrom`, but nothing rescues `MaybeStdin`. Verify a claimed "X requires Y" before
repeating it — three such claims fell in one session.

## serde decision tree

1. Derives + attributes (`rename`, `flatten`, `default`, …) — always first (RS-020).
2. Type has `FromStr`/`Display` → `serde_with::{DeserializeFromStr, SerializeDisplay}` (RS-022).
3. Fallible conversion from a different shape → `#[serde(try_from = "…")]`.
4. One odd field → a small `deserialize_with` fn.
5. Hand-rolled `impl Serialize`/`Deserialize` — only with a one-line comment naming
   what attributes cannot express (RS-020).

`#[serde(transparent)]` is step 0 *for trusted carriers and mere-distinctness types
only* — on a validated type it is a stance violation, not a shortcut.

## Trusted carrier — the other recipe

For protocol strings the counterparty owns — carrying the caller's value to the wire
without an opinion of its own:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize,
         derive_more::Display, derive_more::From)]
#[from(String, &str)]
#[serde(transparent)]
pub struct SurfaceId(String);
```

Do not "harden" one with validation the protocol doesn't specify; that's the inverse
stance mismatch. Note the derive block's `From<String>`/`From<&str>` grants **public
minting** — right for values the caller legitimately owns, wrong for tokens only the
crate or counterparty should mint (a correlation id): see newtypes.md's Construction
authority. A declarative macro (`protocol_string!`) is justified once the crate
declares the same shape several times — one line per domain string beats five restated
derive blocks.

## Errors

The parse error follows RS-005: condition-named (`InvalidName`, `InvalidPaneId`) or
operation-named (`ParseAgentInfoError`) — specific either way, carrying the offending
character/reason. One error per type is the default, not a law: refinements may share
a predicate-named error, and a type with multiple parse forms may split failures
meaningfully. std's leaf convention for library crates is an **opaque struct with
private fields** (`ParseIntError`, `Utf8Error`), reaching for `#[non_exhaustive]` only
where its construction/matching semantics are actually wanted (`IntErrorKind`); a
small enum is fine in a bin crate where the variants are the documentation.
