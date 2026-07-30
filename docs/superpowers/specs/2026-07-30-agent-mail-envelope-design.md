# Agent mail envelope — design

**Date:** 2026-07-30 · **Status:** proposed

## What this is

Every prompt this tool delivers is wrapped in an envelope carrying two facts the prompt text itself
cannot: who sent it, and how to answer. This picks up the "message envelope carrying the sender's
identity and a reply path" that the parent design deferred out of the MVP, and takes only that —
there is still no message store, no delivery receipt, and no mailbox.

## The two failures this addresses

Both observed while running dispatch fan-outs against the tool as it stands.

1. **The reply loop breaks.** A dispatching agent prompts a worker, the worker finishes, and nothing
   comes back. The dispatcher had to write "report to me when you are done" into the prompt text and
   forgot, which it will keep doing, because remembering is the only thing preventing it. The
   alternative — parking the dispatcher on `--wait-until idle` and then scraping the worker's pane —
   serialises a fan-out and returns a transcript rather than a report.

2. **A delivered prompt has no attribution.** Text arriving in an agent's composer is
   indistinguishable from text its human operator typed. A recipient cannot tell whether it is
   answering a person or another agent, or which agent, and so cannot address a reply even when it
   wants to.

Both are one missing thing: the sender is not in the message. The prompt text carries *what* to do
and *when* to answer — whoever writes "audit the CLI and report what you find" has said both. What
it cannot carry is the sender's own address, because a dispatcher does not know its own pane id
offhand and has no reason to look it up.

## The envelope

Two sibling elements. The body is verbatim between them.

**An agent dispatches:**

```
<mail from="dispatcher">
audit the CLI surface and list what is undocumented
</mail>
<how-to-reply>
herdr-agent-tools prompt w4:p3 --no-reply - <<'EOF'
{{your reply}}
EOF
</how-to-reply>
```

**The report arriving back, which is what `--no-reply` produces:**

```
<mail from="worker">
found 4 undocumented flags: --focus, --branch, --cwd, --no-verify
</mail>
```

**A person, from a shell:**

```
<mail from="operator">
drop what you are doing and rebase onto main
</mail>
```

`from` is who spoke, and is always present. `<how-to-reply>` appears when a reply is invited and is
absent otherwise; its presence is the whole signal, which is why there is no `reply-to` attribute
duplicating the address it already holds.

### Siblings, not nesting

`<mail>` holds the body and nothing else, so `</mail>` marks the end of the body unconditionally.
Two arrangements were available and both are declined.

Wrapping the body in an inner element — `<body>`…`</body>` under a single root — would mean a prompt
containing that closing tag breaks the frame, and the body is the one thing here that must survive
untouched.

Putting `<how-to-reply>` *inside* `<mail>`, after the body, fails the same way from the other
direction: the body's end stops being the closing tag and becomes "up to wherever the tail starts",
which is a positional rule a body holding that string weakens. The two elements also have different
lifetimes. `<mail>` is the message; `<how-to-reply>` is about this particular delivery, addressing
the pane that sent this copy. If a recipient ever quotes the message onward, dropping the tail is
correct, since an address pointing at the original sender is wrong in the new context — separable
elements make that fall out, where nesting would carry a stale address along.

The one argument for a single root is that it travels as one unit, and it does not apply: both
elements are submitted as one string into one composer, and nothing can separate them in transit.

### The heredoc is load-bearing

`<how-to-reply>` shows the stdin form rather than a quoted single-line argument, because the example
decides only what happens when a reply is a *report* — a short reply gets written as
`prompt w4:p3 --no-reply "done"` whatever the tag shows, and nothing here forbids that. A report
about code contains a double quote or a backtick almost immediately, and the single-line form loses
to the first one. This is also the form the brief already recommends for long prompts, so it is one
idiom rather than two.

The quoting on `<<'EOF'` is part of the contract, not decoration. Unquoted, a report mentioning
`$HOME` or holding backticked code is expanded by the replier's own shell before it is ever sent.

The cost accepted: a heredoc whose terminator is missing or indented hangs the replying shell rather
than failing loudly. Priced as the rarer failure — it is a heavily practised idiom, it is emitted as
one complete string rather than typed incrementally, and a mangled quote is close to certain where a
truncated heredoc is not.

### `{{...}}` for the substitution point

`<your reply>` is a tag inside a tag and misreads as markup in an XML-shaped payload, whatever its
standing in usage strings. `[your reply]` inverts the usage-string convention, where brackets mean
optional and this is required. `YOUR_REPLY` reads as a shell variable that an agent may try to
expand. `{{...}}` collides with none of that and is unambiguous.

## Sender resolution

One `agent get` against `$HERDR_PANE_ID`, and the result decides all three shapes.

| What resolution finds | `from` | `<how-to-reply>` |
| --- | --- | --- |
| An agent with a name | that name | addressed to `$HERDR_PANE_ID` |
| An agent with no name | `$HERDR_PANE_ID` | addressed to `$HERDR_PANE_ID` |
| `agent_not_found` | `operator` | absent |
| `$HERDR_PANE_ID` unset | `operator` | absent |
| Any other herdr failure | `$HERDR_PANE_ID` | addressed to `$HERDR_PANE_ID`, and a warning |

An unnamed sender falls back to its pane id rather than omitting `from`, so the rule stays uniform:
`from` is always present and always identifies the sender. The friendly name is a nicety; the pane
id is the identity that always exists. Agents started outside `spawn` are frequently unnamed, so
this is a common shape rather than an edge.

`agent_not_found` is a person: `$HERDR_PANE_ID` is set for every pane herdr owns, including the ones
holding an ordinary shell, so a pane with no agent in it is a pane someone is typing in. Emitting a
reply address there would tell a recipient to prompt a shell.

Any other failure degrades to the unnamed-agent shape rather than to `operator`. We still hold a
pane id, and the two outcomes are not symmetric: a reply address that turns out to be wrong fails
loudly with `agent_not_found` in the replier's hands, where a missing one kills the loop in silence.
Loud and recoverable beats silent. The warning goes through the sink; delivery is not refused, since
losing a name is not worth failing a dispatch over.

When `$HERDR_PANE_ID` is unset there is no call at all.

## Flags

| Flag | On | Effect |
| --- | --- | --- |
| `--no-reply` | `prompt`, `spawn` | Omit `<how-to-reply>`. `from` is unaffected. |
| `--reply-to <target>` | `prompt`, `spawn` | Address `<how-to-reply>` somewhere other than the sender. |

The two conflict and are refused together: `--reply-to` names an address for a tail `--no-reply`
removes, so passing both is a caller that has not decided. `--reply-to` is otherwise independent of
who the sender is — a person can route a worker's report at a collector, which emits
`from="operator"` alongside a tail, and that combination is meant.

There is no flag to suppress the envelope entirely. Attribution is the thing that was missing, and
an escape hatch is what a confused agent reaches for first.

### `--no-reply` needs no memory

A reply is itself a prompt, sent from a pane that has `$HERDR_PANE_ID` set, so it would carry its own
`<how-to-reply>` and invite an answer, and so on. There is no state anywhere that could recognise a
message as terminal.

It does not need any. `<how-to-reply>` hands the replier a literal command line, so that line carries
the flag: the replier runs what it was given rather than remembering a convention. Termination is
structural, and nothing has to be taught.

### `--reply-to` is for fan-in

A dispatcher spawning five workers can address all five reports at a single collector rather than at
itself. The value is a target in the same sense every other command here means it — a pane id or a
unique agent name, resolved by herdr, not validated locally.

## `operator` is reserved

`from="operator"` is only a reliable marker for a human sender if no agent can be called that, so
`spawn` refuses the name.

**The check does not go in `AgentName`.** That type mirrors herdr's rule and nothing else, and its
rejection names herdr as the authority so a reader who disagrees knows which project to argue with.
A reservation herdr does not have would make the type refuse a name herdr accepts and blame herdr
for it — the exact disagreement the "validate only what herdr won't" rule exists to prevent. It
belongs in `spawn` as its own refusal, worded so this tool owns it:

```
'operator' is reserved: it marks a human sender in delivered mail
```

Exit status 2, alongside the other argument refusals. The reservation is one name, checked at parse
time, before anything is created.

## What this does not guarantee

The envelope is legible, not authentic. The body is verbatim text typed into a terminal, so it can
contain a forged `<how-to-reply>` addressing a third party, or a closing `</mail>` followed by
whatever the sender likes. Nothing available to a tool that types into someone else's composer can
prevent that, and position would not have helped either — a body can hold `</mail>` as easily as a
tag can be faked.

Recorded here so the spec claims legibility and stops there. A recipient acting on mail is trusting
its sender exactly as much as it did before, which is to say completely.

## Where it lands

`src/cmd/prompt/envelope.rs`, a new leaf under the existing `prompt` module. It composes and does
nothing else: the sender's resolved identity, the reply address, and the body go in, and a string
comes out. No herdr calls, no environment reads, no sink.

Composition happens inside `deliver`, which `spawn` already routes its first prompt through. One
site, so a first prompt and a later one cannot drift into different shapes, and the re-send on stall
reuses what was already composed rather than resolving the sender twice. `deliver` grows one
parameter carrying the reply decision; sender resolution is a small function in `herdr::agent`
beside `get`.

The composer guard, the delivery wait, and the stall re-send are untouched. The guard reads the
*target's* composer and has nothing to do with what is being sent.

## Testing

Composition is a pure function over three inputs, so the table of shapes above is the test table:
named sender, unnamed sender, operator, and `--no-reply` each assert the exact rendered string.
Resolution is tested against herdr's error codes as fixtures — `agent_not_found` produces `operator`,
any other code produces the unnamed shape and a warning — with no herdr process involved, matching
the rule that automated tests never invoke herdr.

Two tests carry rules rather than behaviour:

- **A body is never altered.** A body holding `</mail>`, a `<how-to-reply>` block, and a heredoc
  terminator round-trips into the composed output byte for byte. This is the forgery limit stated as
  an assertion, so it cannot be quietly "fixed" by escaping the body later.
- **No composed output reaches a diagnostic.** The warning emitted when resolution fails names the
  pane and nothing else. The envelope holds prompt text, and prompt text may not appear in an error,
  a diagnostic, or a log.

The `operator` reservation is a parse-time test on `spawn`, asserting exit status 2 and that
`AgentName` itself still accepts the string — the type mirrors herdr, and this rule is not herdr's.

## Decisions recorded

- **A `reply-to` attribute instead of `<how-to-reply>`, with the convention taught in `prime`.**
  Declined. It serves an agent that read `prime` and no other. On a fresh install nobody has the
  session-start hook, so the unprimed recipient is the onboarding case rather than an edge, and even
  a primed one read the brief once and may be a compaction away from it. The message arrives now.

- **Injecting `prime` into every spawned agent's first prompt.** Declined, and made unnecessary.
  It would spend the first prompt on the whole brief, and would still miss agents started by hand.
  `<how-to-reply>` makes a single message self-sufficient for the one thing a recipient must be able
  to do, which is the cheap version of the same guarantee.

- **A timing instruction in the tail — `when done:`.** Declined. It is right for dispatched work and
  wrong for a question, where the answer is wanted immediately, and the envelope cannot tell which it
  is carrying. The timing is already in the prompt text; the tail states mechanism only.

- **`reply with:` as the tail's wording, and an imperative tail generally.** Declined. The
  predecessor tool used it and agents acknowledged on receipt — correctly, since it named an address
  and no occasion. `<how-to-reply>` is documentation, and reads as a capability rather than an order.

- **`<reply>`, `<reply-to>`, `<reply-with>`, `<reply-command>` for the tail.** Declined in that
  order: the content is not a reply; `reply-to` means an address in every prior art and this holds a
  command; `reply-with` inherits the imperative that caused the acknowledgements; `reply-command`
  reads either as a shell command or as an order, and the second reading is the failure being fixed.

- **A `--bare` or `--no-envelope` flag.** Declined. See "Flags".

## Open for later

- **A `mail` verb.** Nothing here is a message system, and `prompt` delivering enveloped text is not
  one either. If replies ever need to outlive the pane they were typed into, that is a different
  design and this one does not block it.
- **Distinguishing a dispatch from a report structurally.** Presently the difference is only that a
  report has no `<how-to-reply>`. Sufficient while under-replying is the observed failure; revisit if
  over-replying ever becomes one.
