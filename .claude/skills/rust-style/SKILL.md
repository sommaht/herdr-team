---
name: rust-style
description: Rust style gate and review procedure. Use when writing, reviewing, or planning Rust code — new modules, error types, constructors, serde/wire shapes, refactors — or when asked to run a style review, style gate, or check Rust against a style guide. Covers From/TryFrom vs try_new, map_err vs #[from], serde attributes vs manual impls, free functions vs methods, module organization, duplication, and unjustified abstractions.
metadata:
  version: "2.0"
---

# Rust Style

## Overview

Two jobs: bind code **generation** to ten rules, and run a **review gate** with stable rule IDs and an honest coverage statement.

Core principle: **reach for what already exists** — std traits, serde attributes, the crate's own surface, its dependencies, its `references/` — before inventing anything. Violating the letter of these rules is violating their spirit.

"What already exists" includes the **sanctioned dependency set**: `serde`, `serde_json`, `serde_with`, `thiserror`, `thiserror-ext`, `derive_more`, `derive_setters`, `getter-methods`, `strum`, `itertools`. These may always be added to a workspace without hesitation — hand-rolling what one of them provides is reinvention, not caution. A repo's `docs/STYLE-GUIDE.md` dependency table may extend this list.

**These rules bind the agent, not the user.** They apply to self-generated choices only. When the user explicitly asks for something a rule discourages, do it — never cite these rules (or YAGNI / rule-of-three) as pushback against the user's stated intent.

## When to use

- Writing or modifying any Rust code (the Generation Rules below apply as you write)
- Asked to review Rust code, run a style gate/review, or check style-guide compliance (Review Procedure)
- Reviewing or writing a plan that contains literal Rust code — **plan code is code**; it ships verbatim, so gate it like a diff

Not for: non-Rust code; formatting (rustfmt's job); relitigating designs the user already decided.

## Generation rules

Apply these while writing. Each is a hard default: follow it, or carry the rule's own
stated exception — there is no third option.

1. Conversions and single-argument validated construction use std traits: `From`, `TryFrom`, `FromStr`. **Do not write `fn try_new`** unless construction takes multiple arguments so no trait fits — and then say so in its doc comment. A validating `try_new(u16)`, `try_new(String)`, etc. IS a `TryFrom` impl; write it as one.
2. Wrote the same `.map_err(...)` closure twice? Stop — write one `From` impl and let `?` do the rest. `map_err` exists to **add context** (path, line, key), never for mechanical wrapping.
3. Never discard an error source with `map_err(|_| ...)` unless the replacement captures strictly more diagnostic data — state why in one line at the site. Preserve causes: wrapper variants carry `#[source]` or `#[from]`.
4. Never stringify a typed error: no `Result<_, String>` past a function boundary, no `.to_string()` when wrapping.
5. Wire shapes use serde derives plus attributes (`rename`, `flatten`, `skip_serializing_if`, `default`, `untagged`, small `deserialize_with` fns). A hand-rolled `impl Serialize`/`Deserialize` requires a one-line comment naming exactly what serde attributes cannot express.
6. Closed sets are enums — never `&str` comparisons, never mode-selecting bools. Parse strings into the enum at the boundary via `FromStr`.
7. A function whose first parameter is a same-crate type that has (or deserves) an `impl` block is a method. Orphan-rule blocked, or the type is foreign? Use an `*Ext` trait ([references/ext-traits.md](references/ext-traits.md)) or move the function to the type's crate — don't leave it free.
8. Every new type, trait, helper, or file gets a one-line justification at introduction — in its doc comment and in your response. If you can't write the line, don't create the thing.
9. Before building, name what you checked: the crate's surface, std, dependencies, and any `references/` directory — at both symbol level (does this trait exist?) and algorithm level (does this computation already exist?).
10. Data crossing a function boundary is a named struct, not a tuple; more than 3 parameters is a params struct.

## Pattern references — read when the work matches

The gate rules are compressed; these docs carry the judgment behind them. When the work
ahead matches a row, **read the reference before writing** — recalling a pattern is not
reading it, and these docs are updated from mined corrections, so memory of them is
stale by definition.

| You're about to… | Read first |
|---|---|
| Add behavior to a type you don't own; write a free fn taking a foreign type | [references/ext-traits.md](references/ext-traits.md) |
| Mint any newtype, or decide its construction/serde/`Borrow` treatment | [references/newtypes.md](references/newtypes.md) |
| Create a string-backed newtype (name, id, token) | [references/string-newtypes.md](references/string-newtypes.md) |
| Wrap a numeric quantity (Hz, dB, counts, rates) | [references/newtypes.md](references/newtypes.md) — unit newtypes |
| Build or extend a crate speaking to an external service, protocol, or CLI | [references/api-crates.md](references/api-crates.md) |
| Design a trait family, wrapper/adapter, or processing chain | [references/composition.md](references/composition.md) |
| Design or extend an error type, or map errors at a CLI/UI boundary | [references/errors.md](references/errors.md) |
| Model lifecycle/mode state, add a bool beside an `Option`, or write a state machine | [references/state-modeling.md](references/state-modeling.md) |

## Review procedure

Copy this checklist and check items off:

```
Gate Progress:
- [ ] 1. Run scripts/check.sh on the target paths
- [ ] 2. Walk references/gate.md rule-by-rule over the diff/files
- [ ] 3. Judgment questions for every new abstraction
- [ ] 4. Load the repo domain layer, if present
- [ ] 5. Emit findings + the coverage statement
```

**Step 1 — mechanical tier.** Run: `bash .claude/skills/rust-style/scripts/check.sh <files-or-dirs>`. Output has two tiers: **violations** (the text proves the predicate; fix or waive) and **candidates** (the rule carries a justified-exception branch — a site comment, a doc justification, an in-progress phase — that the script cannot see; Step 2's walk verifies each candidate's exception and resolves it to violation or cleared). Do not re-derive or skip the script. A violation may be **waived** only with a one-line reason citing its ID (e.g. "RS-001 waived: closure adds call-site context a From impl cannot know") — waivers and candidate resolutions appear in the report, silent drops do not exist.

**Step 2 — walk the gate.** Read [references/gate.md](references/gate.md) and check every judgment-tier rule (marked `judgment`) against the code. Report violations by ID. When the diff contains a pattern-reference shape (the table above — a new newtype, an Ext trait, wire types, a trait family), load that reference and walk the code against it the same way; name it in the coverage statement.

**Step 3 — judgment questions.** For each new type/trait/helper/module in the change: (a) does it carry its one-line justification? (b) name the existing alternative considered (crate surface, std, dependency, references/) and why it doesn't fit. Unanswered = finding RS-050/RS-052.

**Step 4 — domain layer.** If the repo has a domain style layer — `docs/STYLE-DOMAIN.md` or `docs/STYLE-GUIDE.md` (check both names) — read it and walk its rules the same way. Repo domain rules override core rules only where they say so explicitly by ID. Where a repo guide merely *predates* a core rule (no explicit override), the core rule stands and the stale guide passage is reported for update — precedence is decided by the operator's latest decision, not by which document is older.

**Step 5 — report.** Findings first, ordered by severity: `RS-### file:line — one sentence`. Then the mandatory coverage statement:

```
Coverage: violations and candidates via check.sh (each candidate's exception branch
verified by hand); judgment rules RS-004, RS-005, RS-011, RS-013, RS-014, RS-021,
RS-022, RS-032, RS-035, RS-036, RS-037, RS-041, RS-042, RS-050, RS-051,
RS-052, RS-054, RS-063, RS-065 walked by hand.
This gate cannot see: decomposition errors, cross-crate duplication, algorithm-level
reinvention beyond what Step 3 surveyed, and semantic/correctness bugs.
```

A gate that reports "clean" without saying what it didn't check creates false trust — the coverage statement is not optional.

## Verify, don't defend

- Distinguish what you **checked** from what you **recall** — say which is which.
- Challenged on a claim ("are you sure?", "why is this here?")? Open the relevant artifact (the code, references/, docs) before responding. Never cite your own earlier output as authority.
- "Why is this here?" means **explain in one or two sentences, then let the user decide**. It does not mean delete it, and it does not mean silently swap in a different design. If the design is right, say why once; if the user still wants it gone, remove it without relitigating.
- Rejected structure means **less structure**, not different structure. Before adding a replacement abstraction, try writing the two plain lines that do the job.
- A claimed constraint ("serde needs `FromStr`", "clap can't parse `TryFrom` types") is a **checkable artifact** — compile the counter-example before asserting it. Three such claims fell in one session; the one real constraint was found by reading the dependency's source.
- A rejected mechanism does not license weakening semantics. When your glue is rejected as ugly, keep the semantic (e.g. validate-on-read) and find a better mechanism — the sanctioned crates are part of the search space. Trading the semantic away to escape the ugliness is capitulation.
- Deletion directives are scoped to what was named. "Remove the check" does not license removing the field the check read; announce any cascade beyond the named target and wait for the call.

## Red flags — stop and re-read the rules

- You just typed `fn try_new` — rule 1.
- You just pasted a `.map_err(...)` you've written before — rule 2.
- You wrote `map_err(|_| ...)` — even when the new error carries more data, the one-line justification comment at the site is part of the rule — rule 3.
- You're writing `-> Result<..., String>` — rule 4.
- You're hand-building a `serde_json::Map` or writing `impl Serialize` — rule 5.
- You're comparing a `&str` against two known literals — rule 6.
- You're about to copy a sibling module's function — that's RS-034; lift it instead.
- You're explaining a rejection by inventing a new struct — Verify, don't defend.
- You're minting a newtype, Ext trait, wire type, or trait family and haven't opened its pattern reference this session — the table above, before the code.
- The review found nothing and you're about to say "clean" — where's the coverage statement?

## Rationalizations

| Excuse | Reality |
|---|---|
| "try_new is clearer than TryFrom here" | It's the same operation; the trait composes with `?`, generics, and `parse()`. Rule 1 has one exception: multi-arg construction. |
| "map_err everywhere is explicit" | 192 identical closures shipped in one crate; one `From` impl replaced all of them. Explicit ≠ repeated. |
| "It's a small hand-rolled impl, serde attributes are fiddly" | 16 hand-rolled map fns were deleted for 113 lines of derives with identical wire output. Name the attribute gap or derive it. |
| "I'll clean it up after it compiles" | rustfmt fixes whitespace, not shape. The cleanup pass never comes; the gate comes instead. |
| "The reviewer/tests passed it" | fmt, clippy, and green tests were all green on the worst code this rule set was built from. They can't see shape. |
| "This helper doesn't need a justification, it's obvious" | Then the justification is one obvious line. Write it. |

## Files

- **Gate rules**: [references/gate.md](references/gate.md) — all RS-### rules with rationale and examples
- **`*Ext`-trait pattern**: [references/ext-traits.md](references/ext-traits.md) — when method syntax on a foreign type earns its keep (policy home, postfix chains), when it's a wrapper in disguise, and why RS-051 doesn't apply to it
- **Newtype pattern**: [references/newtypes.md](references/newtypes.md) — pick the validation stance first (refinement / trusted carrier / distinctness); construction, serde, and `Borrow` must all agree with it; unit/quantity newtypes and their curated arithmetic surface
- **Composition**: [references/composition.md](references/composition.md) — the Iterator model for your own trait families: by-value wrappers, `&mut T` blanket impls with sibling parity, inline adapters, tuple chains, generalize-the-special-case
- **String newtypes**: [references/string-newtypes.md](references/string-newtypes.md) — the complete derive recipe (view traits, Cow, serde_with), the `Borrow<str>` canonicality test, FromStr-vs-TryFrom, and the serde decision tree
- **API crates**: [references/api-crates.md](references/api-crates.md) — one counterparty typed end to end: the mirror principle (a plain, unopinionated 1:1 facade of the service's structs and payloads), convenience layers composed over the raw tier (never replacing it), required-args/optional-setters fluent surface, validate-only-what-the-server-can't-judge, exact-wire tests and their provenance caveat
- **Errors**: [references/errors.md](references/errors.md) — the four-layer ladder (leaf → operation → aggregate → presentation): naming per layer, per-variant shape honesty, ContextInto for repeated context, one exit-code mapping, fix-first messages
- **State modeling**: [references/state-modeling.md](references/state-modeling.md) — phase enums over flag combinations, variant-owned payloads, `Option<StateData>` as presence + payload, transitions as the only mutation surface, and when flat fields are genuinely orthogonal
- **Mechanical checker**: `scripts/check.sh` — run it, don't reimplement it
- **Pre-commit hook**: `assets/pre-commit` — install with `cp .claude/skills/rust-style/assets/pre-commit .git/hooks/pre-commit && chmod +x .git/hooks/pre-commit`
