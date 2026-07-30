# API crates — one counterparty, typed end to end

An API crate speaks one counterparty's protocol — its requests, replies, vocabulary,
transport, and errors — and nothing else. No policy, no workflow, no side-effect
ordering: those belong to the consumer crate, and the dependency arrow points one way
(consumer → API crate). The exemplar throughout is the `cmux` crate (a typed client for
the cmux terminal's control socket).

> Worked examples are snapshots of that crate — they
> demonstrate the style and may have drifted from the repo, which is fine. The doc is
> self-contained; read the crate for a deep dive when it's present, never as a
> prerequisite.

## Why the abstraction earns its keep

- **One source of truth per wire token.** Every method spelling, parameter key, and
  enum token exists in exactly one place (a `strum` token, a serde attribute). Without
  the crate, tokens get restated at call sites and drift (RS-021's manual wire-maps are
  this failure at function scale).
- **The consumer stays policy-only.** Commands read as domain flow —
  `cmux.pane().create(direction).focus(true).send()?` — with no wire detail in sight.
  This is RS-031's altitude rule enforced by crate boundary.
- **Swappability is real, not theoretical.** The tmux port replaced the entire backend
  (`cmux` crate → `tmux_interface`) while the domain layer's shape survived
  command-for-command. A typed API seam is the difference between a port and a rewrite.
- **Failure splits where it should.** Transport failure (`CallError::Connection`) and
  counterparty rejection (`CallError::Response`) are different animals — in this
  protocol, rejection is an *ordinary outcome that leaves the connection usable*, not
  an error to die on. (That's a property of the protocol, not a universal: some
  protocols desynchronize or terminate on error — know which yours is, and encode it
  in the error type's docs.)

## The mirror principle

The crate is a **plain facade**: as unopinionated a 1:1 mirror of the service as the
language allows. Structs and payloads take the service's own names, shapes, and
nesting — the counterparty's documentation should read as this crate's documentation,
its examples should transliterate term-for-term, and a protocol change upstream should
map to a mechanical diff here. Module *organization* may adapt to Rust convention (the
method-module layout mirrors the protocol because that's useful, not because it's
owed); the *data* mirrors because anything else is an opinion.

Opinions are the failure mode: a "better" field name, a restructured payload, a
convenience that bundles two calls, client-side validation the server already performs
— each is a translation layer the next reader must carry and a drift hazard when the
service moves. Divergences exist only where Rust or serde forces them, and each is
pinned visibly at its site: a `#[serde(rename = "…")]` carrying the exact wire key, or
the one validated value with its stated reason. This is why the wire vocabulary takes
the trusted-carrier stance, and why exact-wire provenance checks are comparisons, not
translations.

This is the good "1:1": mirroring a *service across a wire* supplies the entire typed
layer Rust callers lack. Mirroring another *Rust crate's* API 1:1 supplies nothing —
that is the wrapper module this rule set already declines.

## Convenience composes over the mirror

The mirror is the foundation, not a ban on convenience — a complex process often
deserves a simpler API. The discipline is **compositional and directional**: raw APIs
and models as close to the source as possible come first, and convenience layers are
built by composing them. Never the reverse — don't design the ergonomic surface and
back-fill wire types to serve it, and never let a convenience shape leak into a wire
model.

The exemplar holds both layers: the raw tier (`Connection`, the `Request` types,
`send_and_recv`, pipelining, the event stream) and the fluent tier (`Cmux` → domain
clients → `RequestBuilder`) composed strictly on top. `Cmux::connection()` is the tell
that the layering is honest — the raw tier stays public and reachable, so a consumer
drops down without leaving the crate. Convenience that *replaces* the raw layer, or
privatizes it, has become an opinion.

Whether a given convenience lives in the API crate or the consumer crate is a judgment
call; the test for the API crate is that it composes only this counterparty's raw tier
and stays policy-free. A multi-step workflow that encodes *your* application's policy —
ordering, retries with meaning, domain decisions — belongs in the consumer, however
convenient it would be to ship.

## The design, element by element

**Layout mirrors the protocol.** Each wire method is a module named for the method's
local part (`system.top` → `system::top`, `surface.send_text` → `surface::send_text`);
a method module holds that method's request and reply-unique types; types shared by a
domain live in the domain module; cross-domain types at the root. A reader who knows
the protocol can navigate the crate blind — this is RS-035's "deliberate namespacing as
stated design."

**A `Request` trait binds method to reply.** `const METHOD` plus
`type Output: DeserializeOwned`, with one `to_wire` pairing the request with its
correlation id — so the method spelling can never desynchronize from the typed params.
`Output = serde_json::Value` means "this client does not model the result," never "the
result is empty" — partial modeling is a stated design, not laziness.

```rust
pub trait Request: Serialize {
    const METHOD: Method;                 // strum token = the wire spelling, once
    type Output: DeserializeOwned;        // what the counterparty answers with
}

// in pane::create — every method module names its request `Request`;
// the module path carries the context, so the type name doesn't restate it
#[derive(Serialize, Setters)]
#[setters(strip_option, into)]
pub struct Request {
    #[serde(rename = "source_surface_id", skip_serializing_if = "Option::is_none")]
    pub source_surface: Option<SurfaceSelector>,   // divergence pinned at its site
    pub direction: PaneDirection,                  // required → a method argument
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_command: Option<String>,           // defaulted → a setter
}
```

**Fluent layer: required params are arguments, defaults are setters.** A domain client
is a short-lived view borrowing the connection; its method takes what the counterparty
*requires* (`create(direction)`), so there is no build step that can fail, and
`derive_setters` (`strip_option`, `into`, `generate_delegates`) puts the optional
parameters on the returned `RequestBuilder`. Requests stay as flat as the wire carries
them, so the fluent surface is too. The builder's
`#[must_use = "a request builder does nothing until send is called"]` is the house
example of a *reasoned* must_use.

**Vocabulary is declarative.** Protocol strings and id-vs-ref selectors are two-line
macro invocations (`protocol_string!`, `selector!`) — the trusted-carrier stance from
[newtypes.md](newtypes.md) applied at crate scale: infallible construction,
`#[serde(transparent)]`, no opinion of its own.

**Validate only what the counterparty never gets to judge.** The crate validates
exactly one value — `DividerPosition`, because `serde_json` maps a non-finite `f64` to
`null`, so an unchecked `NaN` would arrive as *absent* rather than as a value the
server could reject. Everything else ships byte-for-byte: the counterparty owns the
policy and reports a better error than the client could invent (a client-side emptiness
check would replace a specific server rejection with a vaguer local one). The floor
rule: **never invent stricter policy than the protocol**; local validation is
legitimate when the rule is protocol-defined and checking before I/O buys something
explicit — a typed error pre-flight, or avoiding an expensive or irreversible request.
State each exception and its reason in the module doc.

**Model replies for your consumers, tolerantly.** serde ignores unmodeled fields, so a
counterparty release that *adds* fields stays compatible, while one that renames a
field the client depends on fails loudly at decode instead of silently reporting
nothing. Model results only where a consumer uses them — `serde_json::Value` is the
*explicit* partial-modeling escape, never an accident, and a crate published for
unknown consumers raises the modeling bar past "what my one caller reads."

**Exact-wire tests pin bytes — but they cannot vouch for tokens.** Every request type
carries a `request_maps_to_exact_wire_json` test asserting the full serialized frame.
These prevent *drift*; they do not prevent *invention* — the `startup_env` bug shipped
with a green exact-wire test happily pinning a key the server never reads (it silently
ignores unknown params). Wire tokens are verified against the counterparty — its source
(file:line) or a live probe — before the test enshrines them.

## When to build one, when not to

Build one when the app speaks a protocol, socket, or external CLI and no typed client
exists. When a maintained API crate already exists (`tmux_interface`), consume it
directly — wrapping an API crate in another same-shape layer is the 1:1 wrapper module,
already declined as a pattern; the accepted residue for cross-cutting policy on a
foreign client is an `*Ext` trait ([ext-traits.md](ext-traits.md)), and the domain
vocabulary still belongs to *your* domain crate, not a wrapper.

A **CLI counterparty** changes the contract surface, not the principle: the mirror
holds, but the elements become argv construction (args, never shell interpolation),
exit-status mapping, stdout/stderr framing, and version probing — this doc's
request/reply and tolerant-serde mechanics are socket/protocol-shaped and don't
transfer literally.
