# State modeling — make illegal states unrepresentable

RS-041 covers closed *inputs* (parse a selector string into an enum at the boundary).
This file covers the state a type holds between calls: lifecycle phases, in-flight
work, presence. The goal is that a stale or contradictory combination of fields cannot
be constructed at all — the same proof-carrying argument newtypes make
([newtypes.md](newtypes.md)), applied to a struct's interior.

> Worked examples are snapshots from real code —
> they demonstrate the style and may have drifted from those repos, which is fine. The
> doc is self-contained; don't go looking for these types in your own workspace.

## The shapes

**Phases are one enum field, not orthogonal flags.** A connection is exactly one of
`Commands`, `AwaitingEventAck`, `Events`, `Terminated` — one `ConnectionState` field,
with the legal and terminal operations documented per variant. Two bools
(`awaiting_ack`, `terminated`) would admit four states where the protocol's phases are
a strict progression — some combinations would be bugs that compile. The tell that
flags have rotted: a comment explaining which combinations "can't happen."

**In-flight work bundles its values into the variant (or one struct).** A filter mid
crossfade holds `Option<Crossfade>` where `Crossfade` owns *every* in-flight value —
progress, the incoming cascade, the target. "Fading" can then never be true beside a
missing or stale ingredient, because the ingredients don't exist outside the bundle.
Scattering them as sibling fields with a `fading: bool` reintroduces every stale
combination the bundle was built to kill.

**`Option<StateData>` is both the presence bit and the payload.** A knob's drag state
is `Option<DragGesture>`; `dragging()` is `is_some()`. Never store a `bool` beside an
`Option` that already answers it — a redundant flag is two sources of truth with an
invariant ("these agree") that nothing enforces.

**Events carry their phase's payload.** A gesture stream is `Begin`/`Change`/`End`
variants each owning exactly that phase's data — not a struct of optional fields plus
a kind discriminant the consumer must cross-check.

**Transitions are methods; dispatchers match exhaustively.** State fields stay private
and mutate only through named transitions (`advance`, `begin_drag`, `poison`) that
encode the legal moves; a true dispatcher `match`es the enum without a wildcard arm,
so a new phase forces every dispatch site to decide (the same new-variant pressure the
exit-code mapping uses in [errors.md](errors.md)).

## When flat fields are right

Genuinely orthogonal state stays flat: fields that vary independently, where every
combination is meaningful (`gain` and `pan`; a smoother's `target` and `current`), gain
nothing from an enum. The test is the invariant: if no sentence of the form "when X is
set, Y must/can't be…" connects two fields, they're orthogonal — the enum is for the
fields such sentences connect. Forcing orthogonal state into variants creates the
inverse disease: every transition rewriting values it doesn't own.
