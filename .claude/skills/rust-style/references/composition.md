# Composition — the Iterator model for your own traits

Rust's `Iterator` is the north star for composing behavior: a small trait, adapters
that hold their inner by value, blanket impls that recover borrowing, chains built
inline where they're consumed, and an associated type binding each implementor to its
one output. The audio DSP stack (`ProcessAudioSample` / `ProcessAudioBlock` / `Setup`)
applies the same model to domain traits; this file generalizes it.

> Worked examples are snapshots from real code — they demonstrate the
> style and may have drifted from that repo, which is fine. The doc is self-contained;
> don't go looking for these traits in your own workspace.

## The model

- **Compile-time composition first.** Generics over `dyn Trait`; trait objects for
  genuinely heterogeneous collections, real runtime boundaries (plugin registries,
  stable ABI, object-safe callbacks), or to cut compile time / API complexity.
  `Box<dyn Fn…>` for *stored* callbacks.
- **Wrappers hold their inner by value** — `struct Wrapper<F> { effect: F }`, exactly as
  `Map<I, F>` holds its iterator — never a lifetime-carrying `&mut F` field. Borrowing
  is recovered through blanket impls on references:

  ```rust
  impl<T: Setup + ?Sized> Setup for &mut T {
      fn setup(&mut self, sample_rate: SampleRate) { T::setup(self, sample_rate) }
  }
  ```

  std does precisely this (`impl<I: Iterator + ?Sized> Iterator for &mut I`) — keep
  the `?Sized` so `&mut dyn Setup` composes too.
  **Parity prompt:** a `&mut F` impl for one sibling trait usually implies one for each
  sibling — a wrapper composing under `ProcessAudioSample` almost always should under
  `ProcessAudioBlock` too. Check the set deliberately; a sibling with genuinely
  different semantics may decline, stated in its doc.
- **Adapters are used inline by default.**
  `PerSample(&mut self.fx).process_audio_block(…)` — constructed and consumed where
  written, like an iterator chain. Store one only when it carries independent state or
  lifetime of its own; `pub` fields added merely to make an adapter storable is the
  smell.
- **Wrappers borrow the smallest coherent state aggregate that keeps signatures
  stable** (`&mut Bus2`, not five of its fields). When simultaneous disjoint borrows
  are genuinely needed, a narrower aggregate is the honest shape — the rule is against
  hand-picked field lists that churn signatures, not against split borrows.
- **Associated types when each implementor has exactly one answer** (`Iterator::Item`);
  generic parameters when a type legitimately implements at many types (`AsRef<T>`).
- **Tuple impls for chains**: `(A, B, C)` processes in order — chain composition at
  compile time, no vec of boxed stages. (RS-040 exempts these explicitly: a
  heterogeneous behavior chain is composition, not data crossing a boundary.)

## Design judgments

- **Scale composes behind the same trait.** A 4-pole filter is its own struct wrapping
  two 2-pole sections, implementing the same trait and owning the split (how Q
  distributes); consumers see only the interface.
- **Generalize the special case; shorthand the common case.** `DryWet` *is* `Parallel`
  with a `Passthrough` arm — keep the one primitive and add the ergonomic constructor
  (`bus2.dry_wet()`), not a sibling type. A wrapper whose behavior differs only by
  strategy takes a mode enum, not a parallel struct. (The Iterator analog: `rev()` and
  `by_ref()` are constructors over primitives, not new trait machinery.) This is the
  constructive half of RS-050/RS-051 — not "don't abstract," but *fold two shapes into
  one*.
- **Bounds live where the concern belongs.** Don't attach `Display` to a coordinate
  type because it's the convenient place to thread formatting — the bound goes on the
  thing that formats. Sign APIs at the most general sound shape; a reusable capability
  is generic over what it decorates, never hard-tied to one consumer.
- **Blanket impls must not have degenerate cases.** If some input class behaves weirdly
  under a blanket impl (`ParamRange for &[T]` and slices with duplicate items),
  implement on the concrete types instead — a little boilerplate beats an impl that
  lies for some inputs.
- **Default trait methods are the correct-by-construction safe path**, overridable for
  performance — Iterator's own `size_hint`/`nth`/`fold` shape. Prefer a correct default
  over a cheaper identity impl; concrete types opt into the fast version.
- **Transforms never mutate their inputs** (`build_*`, `parse_*`, `to_*` take `&self`
  and return values); `&mut self` belongs to genuinely state-bearing types — a filter
  advancing its state is expected, an out-parameter `&mut Vec<T>` is not.

## Relation to the other rules

- **RS-051**: a composition trait has many implementors *by design* — the trait family
  is the second-implementer story. But the rule still bites at birth: don't mint the
  chain trait before two real links exist.
- **[ext-traits.md](ext-traits.md)**: extension traits add your methods to foreign
  types; composition traits are your own family. Both exist to keep call sites postfix
  and policy in one place.
