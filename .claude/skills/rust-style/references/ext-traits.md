# `*Ext` traits — the extension-trait pattern

One pattern, three jobs: method syntax on types you don't own, a single home for
cross-cutting policy, and postfix ergonomics in chains. This is the judgment guide
behind RS-030's "orphan-rule blocked → `*Ext`" and RS-036's foreign-type rule.

> Worked examples are snapshots from real code —
> they demonstrate the style and may have drifted from those repos, which is fine. The
> doc is self-contained; don't go looking for these types in your own workspace.

## What it is

A trait whose only purpose is to attach methods to an existing type, with exactly one
(often blanket) impl:

```rust
/// Runs a built tmux invocation and turns "tmux ran" into "tmux succeeded".
pub trait TmuxExt {
    fn execute(self) -> Result<TmuxOutput, TmuxError>;
}

impl TmuxExt for Tmux { /* status check + stderr → typed error, once */ }
```

Prior art is everywhere the ecosystem touches foreign types: `itertools::Itertools`
(blanket over `Iterator`), `futures::StreamExt`/`FutureExt`, `byteorder::ReadBytesExt`,
`anyhow::Context` (on `Result`/`Option`), std's own `std::os::unix::ffi::OsStrExt` and
`std::os::unix::process::CommandExt`, and the `.into_*()` methods `thiserror_ext::ContextInto`
generates. Two of those crates are already in the sanctioned set — the pattern is as
standard as Rust gets.

## When to reach for it

1. **Foreign type, your behavior.** You keep writing `helper(foreign_value, …)` because
   the orphan rule blocks `impl ForeignType`. The Ext trait restores method position:
   `Tmux::with_command(x).execute()?` instead of `execute(Tmux::with_command(x))?`.
   RS-036's "free function whose primary parameter is a foreign type" *is* this case.
2. **One home for cross-cutting policy.** The method carries a rule every call site must
   share — `TmuxExt::execute` is the single place "exit status checked, stderr's first
   line typed into `TmuxError`" lives. Policy once, at the seam; the operations stay at
   their call sites.
3. **Postfix reads better in chains.** `?`, iterator adapters, and builders all flow
   left-to-right; a free function interrupts the flow. This alone justifies `Itertools`.

Field evidence: in one port of this style onto a different multiplexer, the per-operation
wrapper module was inlined, and
its successor chaining trait was retired for a macro — but `TmuxExt::execute` survived
every teardown, because it was the only piece carrying policy rather than mirroring
operations. The Ext trait is the *residue that earns its keep* when a wrapper layer
dies.

## When not to

- **You own the type** → inherent `impl` block (RS-030). Ext traits are for types you
  cannot open.
- **No shared policy** → wrapping each of a dependency's operations in a same-shape
  method re-creates the 1:1 wrapper layer, just wearing a trait. If a method's honest
  doc line is "calls X", inline the call instead (RS-050's justification test).
- **As an interface** — don't put `T: FooExt` bounds on your own APIs expecting future
  implementers. If generic behavior over multiple implementers is the goal, that's a
  real interface trait and RS-051 applies in full.

## RS-051 does not apply

A single-impl trait is premature *when it poses as an interface*. An Ext trait is not
an interface — it is method syntax plus a policy home, and one blanket impl is its
finished form. Never cite RS-051 against an Ext trait, and never defend a speculative
interface by calling it one. The tell is **use, not impl target** — `Itertools`
blankets every `Iterator`, and real interfaces are sometimes implemented for foreign
types, so ownership of the target proves nothing. What distinguishes them: an Ext
trait adds call syntax and cross-cutting policy and is never consumed as a polymorphic
contract — nothing takes `T: FooExt` bounds awaiting other implementors. An interface
is exactly the thing consumers bound over.

## Mechanics

- **Scope**: the trait must be in scope at the call site — that's a property of traits,
  not a choice. Make it reachable from the crate's established trait-import surface —
  a prelude where one exists, an ordinary re-export where one doesn't; keep it *out* of
  the public facade when it is internal machinery, since a facade re-export would
  advertise plumbing as API.
- **Naming**: `<Type>Ext` when extending one concrete type (`TmuxExt`);
  `<Capability>Ext` when a blanket impl extends a family (`ReadBytesExt: Read`).
- **Sealing** (public library crates only): a private supertrait (`trait Sealed {}`)
  stops downstream impls when the method set must remain yours. Bin crates skip it.
- **Doc comment**: the trait's doc line states the policy it owns *or* the chain
  ergonomics it buys — that line is its RS-050 justification. Either suffices:
  `Itertools` carries no house policy, and its justification is the adapter vocabulary
  itself. If the honest line is just "wraps X", you're looking at a wrapper.
