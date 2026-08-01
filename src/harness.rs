//! What this tool knows about each supported agent CLI: whether it can be prompted right now, and how
//! it wants context handed to it at session start.
//!
//! A caller asks [`readiness`] one question and learns nothing about how it was answered. That it
//! takes a terminal snapshot, which snapshot, and how much of one, are this module's business — a
//! command that knew those would have to be edited every time a harness needed something different
//! to look at. [`AgentHarness::hook`] is the same bargain for the other direction: `prime` hands over
//! a brief and a harness says how its host wants it wrapped.
//!
//! Touches no process and no pane. [`readiness`] is handed the *means* to read and decides what to
//! ask for, so the I/O stays in [`crate::herdr`] and a test supplies a closure returning a literal.
//!
//! **Locating the composer is each harness's own job, and it used to be one shared rule here.**
//! That rule — the lines between the snapshot's last two horizontal rules — is exact for a harness
//! that draws a box and wrong for one that does not. Codex draws no border around its composer at
//! all, and the full-width rules it does draw belong to its transcript, so the shared rule handed
//! back a region of transcript for every Codex agent and the guard silently stopped running. The
//! rule survives in [`prompt_box_range`], which is [`ClaudeCode`]'s locator and the last-resort
//! check that tells one fail-open answer from the other.
//!
//! A new harness is an [`AgentHarness`] impl plus one entry in [`HARNESSES`].

mod claude;
mod codex;

use claude::ClaudeCode;
use codex::Codex;
use serde::Serialize;

// =====================================================================================================================
// Harness
// =====================================================================================================================

/// The hook event a brief answers. One value today, named rather than spelled inline.
const SESSION_START: &str = "SessionStart";

/// `context` in the `SessionStart` envelope: `{"hookSpecificOutput":{…,"additionalContext":…}}`.
///
/// Shared by the impls that say their host reads this shape, so agreeing on an envelope is a call each
/// makes rather than something inherited. What is *not* shared is the claim — see each impl.
///
/// # Errors
///
/// [`serde_json::Error`], which two string fields cannot provoke.
fn session_start(context: &str) -> Result<String, serde_json::Error> {
    serde_json::to_string(&HookEnvelope {
        hook_specific_output: HookPayload {
            hook_event_name: SESSION_START,
            additional_context: context,
        },
    })
}

/// The hook envelope a host parses from stdout.
///
/// A serde struct rather than a hand-assembled string, so the brief is escaped by the same code that
/// escapes everything else. It holds quotes, backticks, and newlines, and hand-rolling that escaping
/// is how a hook ends up receiving invalid JSON.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HookEnvelope<'a> {
    hook_specific_output: HookPayload<'a>,
}

/// What the envelope carries.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HookPayload<'a> {
    /// The event this answers.
    hook_event_name: &'static str,
    /// The brief, which the host injects into the session's context.
    additional_context: &'a str,
}

/// What a harness needs read from a target before it can judge readiness.
///
/// Two values that are meaningless apart, and the caller of [`readiness`] passes them through
/// without interpreting either — which is what keeps the choice of snapshot inside this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Probe {
    /// herdr's `--source`: which of a pane's several renderings to take.
    pub source: &'static str,
    /// herdr's `--format`: whether the rendering keeps its escape sequences.
    pub format: &'static str,
    /// How many lines of it are enough.
    pub lines: u32,
}

impl Default for Probe {
    /// The plain-text bottom-buffer snapshot herdr's own agent detection reads, which is where every
    /// harness known today renders its composer.
    ///
    /// **Eighty lines, because forty was not enough and more than eighty would not help.** Sixty
    /// lines typed into a Codex composer fill the snapshot outright: at forty there is no marker and
    /// no rule left to find, so the composer cannot be located and the guard fails open — on exactly
    /// the draft nobody wants to lose. Eighty reaches it.
    ///
    /// The limit is herdr's, not this number's. `detection` is built from the pane's own row count
    /// and `--lines` only ever shrinks it, so this reaches further only in a pane taller than forty
    /// rows, and a composer taller than its pane cannot be read at all. The guard fails open there
    /// and says so. Claude Code needs none of it: it collapses a paste into
    /// `[Pasted text #1 +13 lines]` chips, so both its borders still fit inside forty.
    fn default() -> Self {
        Self {
            source: "detection",
            format: "text",
            lines: 80,
        }
    }
}

/// The rendering that still carries its escape sequences, read only to tell a suggestion from a draft.
///
/// A second source rather than a second format of the first: `detection` is handed back with its
/// escapes already stripped, whatever `--format` asks for, so the styling this needs exists only in
/// the renderings herdr does not normalize.
///
/// Its line count stays at forty while [`Probe::default`]'s rose, and deliberately: `visible` is the
/// viewport, so a larger number would only misreport how much was actually looked at. A composer
/// this read cannot find already produces the answer that refuses — [`suggestion_only`] returns
/// `false` — so the shorter read costs a draft nothing.
const STYLED: Probe = Probe {
    source: "visible",
    format: "ansi",
    lines: 40,
};

/// A coding-agent CLI whose composer this tool can read.
///
/// Deliberately not a registry of supported agents: herdr's kind list has 21 entries and grows, and
/// restating it here would drift. An entry buys one thing — the ability to read that harness's
/// composer — and a kind absent from it still gets a check through the probe tier in [`readiness`].
///
/// The judgment methods have defaults, so an impl states only what makes it different: the deferred
/// placeholder-vs-typed-text work overrides [`composer_occupied`](Self::composer_occupied), and a
/// harness that renders its composer somewhere else overrides [`probe`](Self::probe). The five
/// *declarations* — kind, marker, where the composer is, hook envelope, and how it spells model and
/// effort — are required, because each is a claim about a specific host that someone has to make
/// deliberately.
pub trait AgentHarness: std::fmt::Debug {
    /// herdr's own kind label for this harness, as `agent get` reports it.
    fn kind(&self) -> &'static str;

    /// The character this harness's composer input begins after.
    fn marker(&self) -> char;

    /// Where this harness draws its composer in a snapshot, as a range over `lines`.
    ///
    /// Required rather than defaulted, and this one is required because of a shipped defect. It was
    /// a single shared rule — the lines between the snapshot's last two horizontal rules — and
    /// sharing it is what broke: Codex draws no border around its composer, and the rules it does
    /// draw belong to its transcript, so that rule handed back a region of transcript for every
    /// Codex agent and the guard stopped running. A harness added later must say where its own
    /// composer sits rather than inherit an answer nobody checked against its rendering.
    ///
    /// `None` when this harness cannot see a composer here, which the caller reads as "not this
    /// harness's, or not visible" and never as "empty".
    fn composer_range(&self, lines: &[&str]) -> Option<std::ops::Range<usize>>;

    /// `context` wrapped the way this harness's host wants it delivered at session start.
    ///
    /// Required rather than defaulted, though both impls call [`session_start`] today. A default would
    /// let a harness added later inherit an envelope nobody checked against its host; making it
    /// required means the author has to say what that host reads, and each impl's doc comment is where
    /// the evidence for that claim goes.
    ///
    /// # Errors
    ///
    /// [`serde_json::Error`] if the envelope cannot be serialized, which two string fields cannot
    /// provoke — the signature carries it rather than panicking on a case that would be a bug here.
    fn hook(&self, context: &str) -> Result<String, serde_json::Error>;

    /// The flags this harness's CLI expresses `model` and `effort` as.
    ///
    /// Required rather than defaulted, for the reason [`hook`](Self::hook) is: a harness added later
    /// must not inherit a spelling nobody checked against its CLI. The two fields are independent —
    /// either may be set without the other — and an empty vector is the answer only when neither is.
    ///
    /// A kind absent from [`HARNESSES`] has no answer at all, which is why the config layer drops an
    /// agent that asks for these under one rather than starting it without them.
    fn tuning(&self, model: Option<&str>, effort: Option<&str>) -> Vec<String>;

    /// What this harness needs read in order to answer.
    ///
    /// A harness overriding this can only be reached through its kind: the probe tier has no harness
    /// yet when it decides what to read, so it uses [`Probe::default`] and only harnesses content
    /// with that can be identified by probing.
    fn probe(&self) -> Probe {
        Probe::default()
    }

    /// Whether this harness's composer holds unsubmitted input.
    ///
    /// `None` when the body is not recognizable as this harness's composer — no non-empty line at
    /// all, or a leading line that does not begin with its marker. The caller then fails open, so
    /// this must not guess: answering `Some(true)` for a body it does not understand would refuse
    /// delivery on evidence it does not have.
    fn composer_occupied(&self, body: &[&str]) -> Option<bool> {
        occupied_after(self.marker(), body)
    }
}

/// Every harness this build can read, in probe order.
pub const HARNESSES: [&dyn AgentHarness; 2] = [&ClaudeCode, &Codex];

/// Resolves herdr's kind label back to the harness that reads it.
///
/// `None` for a kind this build does not know, which is the common case for [`readiness`] — the caller
/// then probes. A caller that *named* a harness rather than reading one off a pane treats `None` as a
/// bad argument instead.
pub fn by_kind(kind: &str) -> Option<&'static dyn AgentHarness> {
    HARNESSES.into_iter().find(|harness| harness.kind() == kind)
}

/// Every kind a caller may name, for spelling the choices in a rejection.
///
/// Derived from [`HARNESSES`] rather than listed, so adding a harness cannot leave an error message
/// advertising one set of names while [`by_kind`] accepts another.
pub fn kinds() -> Vec<&'static str> {
    HARNESSES.into_iter().map(AgentHarness::kind).collect()
}

// =====================================================================================================================
// Composer
// =====================================================================================================================

/// What a detection snapshot says about the target's composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Composer {
    /// The composer was located and holds nothing.
    Empty,
    /// The composer was located and holds unsent text.
    Occupied,
    /// No harness could locate a composer, and neither could the generic two-rule search.
    NoPromptBox,
    /// The generic two-rule search found a box, but no harness recognized what is in it.
    UnknownMarker,
}

impl Composer {
    /// The warning this answer carries, for the two that fail open.
    ///
    /// Neither says what the composer holds — that is someone's half-written message.
    pub fn warning(self) -> Option<&'static str> {
        match self {
            Self::NoPromptBox => Some("could not locate the composer in the target's snapshot; delivering unguarded"),
            Self::UnknownMarker => {
                Some("the target's composer uses a prompt marker this build does not know; delivering unguarded")
            }
            Self::Empty | Self::Occupied => None,
        }
    }
}

/// Whether the target can be prompted right now.
///
/// `read` is the means, not the decision: it is handed the source and line count the resolved harness
/// asked for and returns what it read. A caller therefore never names a snapshot, and a harness that
/// needs to look somewhere else changes nothing outside this module. Any read failure is the
/// caller's error type, returned untouched.
///
/// `kind` is what `agent get` reported, which selects the harness. Resolution is two-tiered so an
/// unfamiliar kind still gets a check rather than none: the reported kind's own harness answers if
/// it recognizes a composer, and anything else — an unknown kind, or a known one whose pane rendered
/// something its harness cannot read — falls back to whichever harness does recognize one.
///
/// **Each attempt is atomic: one harness locates the composer, and that same harness decides whether
/// it holds anything.** Splitting those is Finding 2. [`HARNESSES`] is Claude-first, and Claude's
/// two-rule search *succeeds* on a Codex snapshot — by bounding a region of Codex's transcript
/// between two rules that Codex drew after tool calls. A search that took the first locator to
/// answer would hand that region to Codex's recognition and never reach the live marker below it.
///
/// **Fails open** in exactly two cases — no composer could be located, or one was and no harness
/// recognized it. Both deliver anyway, with a warning, because a tool that refused every pane it
/// could not parse would be unusable the first time a harness changed its rendering.
///
/// The answer is a snapshot, not a lock: a human can start typing between the read and the
/// submission. Narrowing that window further would need something herdr does not expose.
///
/// # Errors
///
/// Returns whatever `read` returned.
pub fn readiness<E>(
    kind: Option<&str>,
    read: impl Fn(&'static str, &'static str, u32) -> Result<String, E>,
) -> Result<Composer, E> {
    let harness = kind.and_then(by_kind);
    let probe = harness.map_or_else(Probe::default, AgentHarness::probe);
    let snapshot = read(probe.source, probe.format, probe.lines)?;
    let lines: Vec<&str> = snapshot.lines().collect();

    // The reported kind's own harness first, then every other.
    let recognized = harness
        .and_then(|harness| recognize(harness, &lines))
        .or_else(|| HARNESSES.into_iter().find_map(|harness| recognize(harness, &lines)));

    let Some((harness, occupied)) = recognized else {
        // Nothing recognized a composer, and the generic two-rule search survives only here — to
        // say which of the two fail-open answers this is. A box nobody could read is an unknown
        // marker; no box at all is no box at all. Neither is reported as `Empty`, because claiming
        // an unidentified composer is empty is a guarantee this did not earn.
        return Ok(if prompt_box_range(&lines).is_some() {
            Composer::UnknownMarker
        } else {
            Composer::NoPromptBox
        });
    };

    Ok(match occupied {
        // Content in the box is not yet a draft: a harness draws its own suggestions there, and the
        // plain rendering shows them exactly as it shows typed text. Confirmed against the styled
        // rendering before refusing, which costs a second read on the refusal path alone. The
        // harness that recognized the plain composer locates the styled one too, so the second read
        // cannot select a different region than the first.
        true if suggestion_only(harness, &read(STYLED.source, STYLED.format, STYLED.lines)?) => Composer::Empty,
        true => Composer::Occupied,
        false => Composer::Empty,
    })
}

/// One harness's whole answer: its own locator, then its own reading of what it located.
///
/// `None` if either half declines, which is what keeps an attempt atomic — a harness that can find a
/// region but not recognize what is in it has not answered, and the next harness gets the snapshot
/// rather than that region.
fn recognize(harness: &'static dyn AgentHarness, lines: &[&str]) -> Option<(&'static dyn AgentHarness, bool)> {
    let range = harness.composer_range(lines)?;
    Some((harness, harness.composer_occupied(&lines[range])?))
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// Whether anything follows `marker` in a composer's body — the half every harness shares.
///
/// `None` when the body is not this marker's composer: no non-empty line at all, or a leading line
/// that does not begin with it. Both are a failure to *identify* the composer, distinct from
/// identifying an empty one, and the caller renders them differently.
///
/// The strip is fallible on purpose. Resolving a marker from the reported kind proves nothing about
/// what the pane actually rendered, so a marker that does not match means this harness cannot answer
/// — not that the line counts as text. Treating a failed strip as content would report every
/// unexpected rendering as occupied, refusing delivery on evidence the guard never had.
fn occupied_after(marker: char, body: &[&str]) -> Option<bool> {
    let head = body.iter().position(|line| !line.trim().is_empty())?;
    let after_marker = body[head].trim_start().strip_prefix(marker)?;

    Some(
        !after_marker.trim().is_empty()
            || body
                .iter()
                .enumerate()
                .any(|(index, line)| index != head && !line.trim().is_empty()),
    )
}

/// Whether the box holds content and every character of it is faint.
///
/// Faint — SGR 2 — is how a harness draws text it wrote for itself: a suggested next action offered
/// to an operator who has been away. Stripped of styling it is indistinguishable from a draft, which
/// is why the plain tier cannot decide this and why refusing on its answer alone rejected panes
/// nobody had typed into.
///
/// Faintness rather than styling in general, because a draft can be styled too: a harness colours a
/// skill name the operator typed. Colour is emphasis and faint is its opposite, so only faint can
/// stand for text nobody wrote.
///
/// Answers `false` for a composer it cannot find or one whose content is not wholly faint — the
/// caller then refuses, so every uncertainty here keeps the guard rather than dropping it. That is
/// also what makes the shorter [`STYLED`] read safe: a draft tall enough to push the marker out of
/// the viewport is refused rather than waved through.
///
/// `harness` is the one that recognized the *plain* composer, so both reads select the same region
/// by the same rule. Locating this one generically would let the styled tier answer about a
/// different part of the screen than the tier that asked the question.
fn suggestion_only(harness: &dyn AgentHarness, styled: &str) -> bool {
    let read: Vec<StyledLine> = styled.lines().map(StyledLine::read).collect();
    let plain: Vec<&str> = read.iter().map(|line| line.text.as_str()).collect();
    let Some(body) = harness.composer_range(&plain) else {
        return false;
    };

    let mut content = false;
    for line in &read[body] {
        for (character, faint) in line.text.chars().zip(&line.faint) {
            // The marker is the box's own furniture rather than anyone's text, and it is drawn
            // unfaint beside a suggestion — so counting it would answer `false` every time.
            if character.is_whitespace() || HARNESSES.into_iter().any(|harness| harness.marker() == character) {
                continue;
            }
            if !*faint {
                return false;
            }
            content = true;
        }
    }
    content
}

/// One line of a styled snapshot: its text, and whether each character of that text was faint.
///
/// The two are parallel by construction — a character is pushed to both or to neither — so a caller
/// can ask about the box's content after locating it in the text alone.
struct StyledLine {
    /// The line with its escape sequences removed.
    text: String,
    /// Whether the character at the same position was drawn faint.
    faint: Vec<bool>,
}

impl StyledLine {
    /// Splits a line into its text and its faintness, following the SGR sequences that set it.
    ///
    /// Only the attribute this asks about is tracked. A colour is read and discarded rather than
    /// ignored, because it is `0` and `22` — the resets — that end a faint run, and a parser that
    /// skipped sequences it did not care about would miss them.
    fn read(line: &str) -> Self {
        let mut text = String::new();
        let mut faint = Vec::new();
        let mut drawn_faint = false;
        let mut characters = line.chars().peekable();

        while let Some(character) = characters.next() {
            if character != '\u{1b}' {
                text.push(character);
                faint.push(drawn_faint);
                continue;
            }
            // `ESC [ <parameters> <final byte>`. Anything else is not a sequence this reads, and
            // dropping the escape alone leaves the rest as the text it already is.
            if characters.peek() != Some(&'[') {
                continue;
            }
            characters.next();
            let mut parameters = String::new();
            for character in characters.by_ref() {
                if character.is_ascii_alphabetic() {
                    if character == 'm' {
                        drawn_faint = faintness_after(drawn_faint, &parameters);
                    }
                    break;
                }
                parameters.push(character);
            }
        }

        Self { text, faint }
    }
}

/// Whether text is faint after an SGR sequence carrying `parameters`, given that it was before.
///
/// An empty parameter list is `0`, which is what `ESC[m` means and what a harness emits before it
/// sets the colour it actually wanted.
///
/// A colour's own arguments are consumed rather than scanned, because `38;2;<r>;<g>;<b>` selects
/// twenty-four-bit foreground and that `2` is the colour space — reading it as the faint attribute
/// called every coloured draft a suggestion, which is the one mistake this whole check exists to
/// avoid.
fn faintness_after(current: bool, parameters: &str) -> bool {
    let mut faint = current;
    let mut values = parameters.split(';');

    while let Some(parameter) = values.next() {
        match parameter {
            "" | "0" | "22" => faint = false,
            "2" => faint = true,
            "38" | "48" | "58" => match values.next() {
                // `5;<n>` is one indexed colour; `2;<r>;<g>;<b>` is three channels.
                Some("5") => drop(values.next()),
                Some("2") => {
                    values.next();
                    values.next();
                    values.next();
                }
                _ => {}
            },
            _ => {}
        }
    }

    faint
}

/// The lines between the last two horizontal rules.
///
/// [`ClaudeCode`]'s locator, and the last-resort check in [`readiness`] that tells one fail-open
/// answer from the other. It is the same region herdr's own agent detection extracts, and it was
/// this module's one shared rule until Finding 2 showed that a harness drawing no box is not served
/// by a rule about boxes — see [`AgentHarness::composer_range`].
fn prompt_box_range(lines: &[&str]) -> Option<std::ops::Range<usize>> {
    let mut rules = lines
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, line)| is_horizontal_rule(line))
        .map(|(index, _)| index);
    let bottom = rules.next()?;
    let top = rules.next()?;
    Some(top + 1..bottom)
}

/// Whether a line is one of the box's borders.
///
/// A rule is a line whose leading run of `─` is either the whole trimmed line or at least three
/// characters long — which is how a labelled top border (`──── repo ──`) still counts as one, while
/// a single dash followed by prose does not.
fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    let rule_characters = trimmed.chars().take_while(|character| *character == '─').count();
    if rule_characters == 0 {
        return false;
    }
    let suffix: String = trimmed.chars().skip(rule_characters).collect();
    suffix.trim_start().is_empty() || rule_characters >= 3
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// One scenario's two renderings, the kind `agent get` reported for it, and its answer.
    ///
    /// Both files, because the guard reads both and they are not the same text — the styled one is
    /// the only place a harness's own suggestion is distinguishable from someone's draft. `styled`
    /// is absent only for a capture taken without one, where the plain text stands in: those files
    /// carry no escapes at all, so nothing in them can be mistaken for faint.
    ///
    /// `fixtures/composer/README.md` is the index of what each is there to prove.
    struct Case {
        name: &'static str,
        kind: Option<&'static str>,
        plain: &'static str,
        styled: Option<&'static str>,
        expected: Composer,
    }

    /// The two renderings of one scenario, by name.
    macro_rules! pair {
        ($name:literal) => {
            (
                include_str!(concat!("../fixtures/composer/", $name, ".txt")),
                Some(include_str!(concat!("../fixtures/composer/", $name, ".ansi.txt"))),
            )
        };
    }

    const CASES: [Case; 22] = [
        // -- Codex ----------------------------------------------------------------------------
        Case {
            // Finding 2's regression. The rules in this snapshot are transcript, drawn after tool
            // calls, and the composer is below them — so the answer must come from the marker and
            // never from whatever lies between the last two rules.
            name: "a Codex composer holding only its own placeholder, under transcript rules",
            kind: Some("codex"),
            plain: pair!("codex-empty").0,
            styled: pair!("codex-empty").1,
            expected: Composer::Empty,
        },
        Case {
            name: "a typed Codex line is unsent text",
            kind: Some("codex"),
            plain: pair!("codex-draft").0,
            styled: pair!("codex-draft").1,
            expected: Composer::Occupied,
        },
        Case {
            name: "a Codex draft continuing onto later lines counts as a whole",
            kind: Some("codex"),
            plain: pair!("codex-multiline").0,
            styled: pair!("codex-multiline").1,
            expected: Composer::Occupied,
        },
        Case {
            // The honest "cannot see it" case: sixty typed lines fill the read and push the marker
            // out of it, so there is nothing left to locate and the guard fails open.
            name: "a paste taller than the read hides the composer entirely",
            kind: Some("codex"),
            plain: pair!("codex-pasted").0,
            styled: pair!("codex-pasted").1,
            expected: Composer::NoPromptBox,
        },
        Case {
            // The same paste in the same pane, read far enough back to reach the marker.
            name: "the same paste read past forty lines is the draft it is",
            kind: Some("codex"),
            plain: pair!("codex-pasted-tall").0,
            styled: pair!("codex-pasted-tall").1,
            expected: Composer::Occupied,
        },
        Case {
            name: "a working Codex agent whose composer nobody has touched",
            kind: Some("codex"),
            plain: pair!("codex-working").0,
            styled: pair!("codex-working").1,
            expected: Composer::Empty,
        },
        Case {
            // The queued banner opens with a block marker, and it sits *above* the live composer —
            // so it must not disqualify the marker below it.
            name: "queued messages above an untouched Codex composer",
            kind: Some("codex"),
            plain: pair!("codex-working-queued").0,
            styled: pair!("codex-working-queued").1,
            expected: Composer::Empty,
        },
        Case {
            name: "queued messages above a Codex composer that also holds a draft",
            kind: Some("codex"),
            plain: pair!("codex-working-queued-draft").0,
            styled: pair!("codex-working-queued-draft").1,
            expected: Composer::Occupied,
        },
        Case {
            // Codex draws this menu *below* the composer, where the blank-line bound keeps it out.
            // The `/` itself is typed, so the composer really does hold unsent text.
            name: "a Codex command menu below a composer holding the slash that opened it",
            kind: Some("codex"),
            plain: pair!("codex-slash-menu").0,
            styled: pair!("codex-slash-menu").1,
            expected: Composer::Occupied,
        },
        Case {
            name: "a Codex file picker below a composer holding the path being typed",
            kind: Some("codex"),
            plain: pair!("codex-at-menu").0,
            styled: pair!("codex-at-menu").1,
            expected: Composer::Occupied,
        },
        Case {
            name: "an idle Codex composer that draws both borders holds nothing",
            kind: Some("codex"),
            plain: include_str!("../fixtures/composer/codex-bordered-empty.txt"),
            styled: None,
            expected: Composer::Empty,
        },
        Case {
            name: "a draft on a later line of a bordered Codex box counts too",
            kind: Some("codex"),
            plain: include_str!("../fixtures/composer/codex-bordered-multiline.txt"),
            styled: None,
            expected: Composer::Occupied,
        },
        // -- Claude Code ----------------------------------------------------------------------
        Case {
            name: "an idle Claude Code composer holds nothing",
            kind: Some("claude"),
            plain: pair!("claude-empty").0,
            styled: pair!("claude-empty").1,
            expected: Composer::Empty,
        },
        Case {
            name: "a half-written line beside the marker is unsent text",
            kind: Some("claude"),
            plain: pair!("claude-occupied").0,
            styled: pair!("claude-occupied").1,
            expected: Composer::Occupied,
        },
        Case {
            name: "a Claude Code draft continuing onto later lines counts as a whole",
            kind: Some("claude"),
            plain: pair!("claude-multiline").0,
            styled: pair!("claude-multiline").1,
            expected: Composer::Occupied,
        },
        Case {
            // The contrast with `codex-pasted`: this harness collapses a paste into chips, so both
            // borders and the live marker still fit inside a forty-line window.
            name: "a paste Claude Code collapsed into chips is still a draft",
            kind: Some("claude"),
            plain: pair!("claude-pasted").0,
            styled: pair!("claude-pasted").1,
            expected: Composer::Occupied,
        },
        Case {
            name: "a working Claude Code agent whose composer nobody has touched",
            kind: Some("claude"),
            plain: pair!("claude-working").0,
            styled: pair!("claude-working").1,
            expected: Composer::Empty,
        },
        Case {
            name: "a working Claude Code agent with a draft waiting",
            kind: Some("claude"),
            plain: pair!("claude-working-draft").0,
            styled: pair!("claude-working-draft").1,
            expected: Composer::Occupied,
        },
        Case {
            // This harness writes `Press up to edit queued messages` into the composer itself,
            // faint — so only the styled tier can tell it from a draft.
            name: "a queued-message notice drawn inside the composer is not a draft",
            kind: Some("claude"),
            plain: pair!("claude-working-queued").0,
            styled: pair!("claude-working-queued").1,
            expected: Composer::Empty,
        },
        Case {
            // The marker is echoed into the transcript above the live composer, which the two-rule
            // locator ignores by construction — and is why this harness gets no marker scan.
            name: "markers echoed into a Claude Code transcript are not the composer",
            kind: Some("claude"),
            plain: pair!("claude-transcript").0,
            styled: pair!("claude-transcript").1,
            expected: Composer::Empty,
        },
        Case {
            // Drawn *above* the composer here, where Codex draws its own below.
            name: "a Claude Code command menu above a composer holding the slash that opened it",
            kind: Some("claude"),
            plain: pair!("claude-slash-menu").0,
            styled: pair!("claude-slash-menu").1,
            expected: Composer::Occupied,
        },
        Case {
            name: "a Claude Code file picker above a composer holding the path being typed",
            kind: Some("claude"),
            plain: pair!("claude-at-menu").0,
            styled: pair!("claude-at-menu").1,
            expected: Composer::Occupied,
        },
    ];

    /// The fixtures that belong to neither harness, kept apart because neither kind explains them.
    const UNCLAIMED: [Case; 2] = [
        Case {
            name: "a pane hosting no agent at all fails open",
            kind: Some("claude"),
            plain: include_str!("../fixtures/composer/no-rules.txt"),
            styled: None,
            expected: Composer::NoPromptBox,
        },
        Case {
            name: "a box whose marker matches nothing known fails open",
            kind: None,
            plain: include_str!("../fixtures/composer/unknown-marker.txt"),
            styled: None,
            expected: Composer::UnknownMarker,
        },
    ];

    /// The last `lines` lines, which is what herdr's `--lines` hands back.
    ///
    /// Applied to every fixture read rather than only the pasted pair, so a probe raised or lowered
    /// changes what these tests see — which is the whole reason the pasted pair proves anything.
    fn tail(snapshot: &str, lines: u32) -> String {
        let all: Vec<&str> = snapshot.lines().collect();
        let start = all.len().saturating_sub(usize::try_from(lines).unwrap_or(usize::MAX));
        all[start..].join("\n")
    }

    /// `readiness` against one scenario's two files instead of a pane.
    ///
    /// The whole reason the read is a closure: this module never performs I/O, so a test hands it a
    /// literal and the read cannot fail. Each rendering is served from its own capture — faking the
    /// plain one by stripping escapes off the styled one would test a text herdr never produces,
    /// since `detection` and `visible` differ in what they wrap and how far back they reach.
    fn against_pair(kind: Option<&str>, plain: &str, styled: Option<&str>) -> Composer {
        readiness(kind, |source, _format, lines| {
            let snapshot = if source == STYLED.source {
                styled.unwrap_or(plain)
            } else {
                plain
            };
            Ok::<_, std::convert::Infallible>(tail(snapshot, lines))
        })
        .expect("reading a fixture is infallible")
    }

    /// `readiness` against a fixture with no styled capture of its own.
    fn against(kind: Option<&str>, snapshot: &str) -> Composer {
        against_pair(kind, snapshot, None)
    }

    /// `readiness` against a *styled* fixture, standing in for both renderings herdr offers.
    ///
    /// Kept for the two captures that exist only as styled text: the plain tier is served the same
    /// snapshot with its escapes removed, which is what herdr's own `detection` source hands back.
    /// Every scenario captured as a pair uses [`against_pair`] instead.
    fn against_styled(kind: Option<&str>, styled: &'static str) -> Composer {
        readiness(kind, |_source, format, _lines| {
            Ok::<_, std::convert::Infallible>(if format == STYLED.format {
                styled.to_owned()
            } else {
                styled
                    .lines()
                    .map(|line| StyledLine::read(line).text)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        })
        .expect("reading a fixture is infallible")
    }

    /// A suggestion the harness drew for itself is not someone's unsent message.
    ///
    /// The reported failure: a composer left idle long enough acquires a suggested next action,
    /// which the plain snapshot renders indistinguishably from a draft — so `prompt` refused a pane
    /// whose composer its operator had never touched.
    #[test]
    fn a_faint_suggestion_in_the_box_is_not_a_draft() {
        assert_eq!(
            against_styled(
                Some("claude"),
                include_str!("../fixtures/composer/claude-suggestion.txt")
            ),
            Composer::Empty
        );
    }

    /// The discriminator is faintness, not styling at all.
    ///
    /// A draft naming a skill is *coloured*, which an earlier reading of this bug mistook for the
    /// mark of a suggestion. Colour means emphasis and faint means the opposite, so only faint can
    /// stand for text nobody typed.
    #[test]
    fn a_coloured_draft_is_still_a_draft() {
        assert_eq!(
            against_styled(
                Some("claude"),
                include_str!("../fixtures/composer/claude-highlighted-draft.txt")
            ),
            Composer::Occupied
        );
    }

    #[test]
    fn the_guard_answers_each_fixture_the_way_the_design_says() {
        for case in CASES.iter().chain(&UNCLAIMED) {
            assert_eq!(
                against_pair(case.kind, case.plain, case.styled),
                case.expected,
                "{}",
                case.name
            );
        }
    }

    /// Which Codex is installed must not change the answer.
    ///
    /// One draws both borders around its composer and one draws neither, and the same scenario has
    /// to read the same either way — otherwise upgrading Codex silently turns the guard off.
    #[test]
    fn the_bordered_and_unbordered_codex_renderings_of_a_scenario_agree() {
        for (bordered, (plain, styled), scenario) in [
            (
                include_str!("../fixtures/composer/codex-bordered-empty.txt"),
                pair!("codex-empty"),
                "an untouched composer",
            ),
            (
                include_str!("../fixtures/composer/codex-bordered-multiline.txt"),
                pair!("codex-multiline"),
                "a draft across several lines",
            ),
        ] {
            assert_eq!(
                against(Some("codex"), bordered),
                against_pair(Some("codex"), plain, styled),
                "{scenario}"
            );
        }
    }

    /// The raised probe is what reaches a composer a paste pushed out of the window.
    ///
    /// The pair is the proof, not either file alone: both are the same sixty-line draft in the same
    /// sixty-five-row pane, and the only difference is how far back the read asked for. Both are
    /// served through a closure that truncates the way herdr's `--lines` does, so lowering the probe
    /// back to forty turns the second answer into the first and fails here.
    #[test]
    fn a_paste_that_hides_the_composer_at_forty_lines_is_found_at_eighty() {
        let short = include_str!("../fixtures/composer/codex-pasted.txt");
        let tall = include_str!("../fixtures/composer/codex-pasted-tall.txt");

        assert_eq!(against(Some("codex"), short), Composer::NoPromptBox, "nothing to see");
        assert_eq!(
            against_pair(
                Some("codex"),
                tall,
                Some(include_str!("../fixtures/composer/codex-pasted-tall.ansi.txt"))
            ),
            Composer::Occupied,
            "the same draft, read far enough back to find its marker"
        );

        // The two really are one screen read twice, which is what makes the comparison mean
        // anything: the shorter file *is* the tail of the longer one.
        assert_eq!(tail(tall, 40), tail(short, 40));
        assert!(Probe::default().lines > 40, "or the pair proves nothing");
    }

    /// Claude Code needs no raised probe for the same scenario, and that is the contrast.
    #[test]
    fn a_paste_claude_code_collapsed_into_chips_is_still_found_inside_forty_lines() {
        let plain = tail(include_str!("../fixtures/composer/claude-pasted.txt"), 40);
        let styled = include_str!("../fixtures/composer/claude-pasted.ansi.txt");

        let answer = readiness(Some("claude"), |source, _format, lines| {
            Ok::<_, std::convert::Infallible>(if source == STYLED.source {
                tail(styled, lines)
            } else {
                plain.clone()
            })
        })
        .expect("reading a fixture is infallible");

        assert_eq!(answer, Composer::Occupied);
    }

    /// A marker with transcript below it is transcript, and answering from it would be a lie.
    ///
    /// The only case that exercises the block-marker rejection: the untruncated fixture never asks,
    /// because the bottom-up scan finds the live marker first. Cut the snapshot above that live
    /// marker and the newest candidate left is an echo of an already-sent message, with the agent's
    /// answer to it below. Reading that as someone's draft would refuse every message to this agent
    /// and name a draft that does not exist.
    #[test]
    fn a_codex_marker_with_a_block_marker_below_it_is_a_past_message_rather_than_a_draft() {
        let whole = include_str!("../fixtures/composer/codex-working.txt");
        let above_the_composer: Vec<&str> = whole.lines().take(11).collect();

        assert!(
            above_the_composer.iter().any(|line| line.starts_with('›')),
            "there is a candidate marker to reject"
        );
        assert_eq!(Codex.composer_range(&above_the_composer), None);

        assert_eq!(
            against(Some("codex"), &above_the_composer.join("\n")),
            Composer::NoPromptBox
        );
    }

    /// One rule and a footer is not a composer, and must not be read as one.
    #[test]
    fn a_snapshot_whose_only_rule_sits_above_a_footer_holds_no_composer() {
        let snapshot = "  ⏺ Read src/main.rs (42 lines)\n\
                        \n\
                        ────────────────────────────────────\n\
                        \n\
                        \x20 ⏵⏵ accept edits on      17k tokens";

        for kind in [Some("claude"), Some("codex"), None] {
            assert_eq!(against(kind, snapshot), Composer::NoPromptBox, "{kind:?}");
        }
    }

    /// The caller reads what the resolved harness asked for, and nothing else chooses it.
    #[test]
    fn the_read_is_the_one_the_resolved_harness_asked_for() {
        // A `Cell` because the read is now called more than once on some paths, so the closure has
        // to be `Fn` rather than `FnOnce`.
        let asked = std::cell::Cell::new(None);
        let _ = readiness(Some("claude"), |source, format, lines| {
            asked.set(Some((source, format, lines)));
            Ok::<_, std::convert::Infallible>(String::new())
        });

        assert_eq!(
            asked.get(),
            Some((Probe::default().source, Probe::default().format, Probe::default().lines))
        );
    }

    /// A read failure is the caller's, returned untouched rather than folded into an answer.
    #[test]
    fn a_failed_read_is_not_reported_as_a_composer_answer() {
        let answer = readiness(Some("claude"), |_source, _format, _lines| Err("the pane went away"));
        assert_eq!(answer, Err("the pane went away"));
    }

    #[test]
    fn both_fail_open_answers_carry_a_warning_and_neither_success_answer_does() {
        // Failing open is the whole point: a tool that refused every pane it could not parse would
        // be unusable the first time a harness changed its rendering. Delivered without the
        // guarantee, and said so — never guessed at.
        assert!(Composer::NoPromptBox.warning().is_some());
        assert!(Composer::UnknownMarker.warning().is_some());
        assert!(Composer::Empty.warning().is_none());
        assert!(Composer::Occupied.warning().is_none());
    }

    /// Each attempt is one harness's locator *and* that same harness's recognition.
    ///
    /// The case that makes it load-bearing, and Finding 2 in miniature: Claude's two-rule locator
    /// succeeds on a Codex snapshot, because Codex draws full-width rules in its transcript after a
    /// tool call. A search that took the first locator to answer would hand that region of
    /// transcript to Codex's recognition and never reach the live marker below it.
    #[test]
    fn a_locator_that_succeeds_for_the_wrong_harness_does_not_get_to_answer() {
        let codex = include_str!("../fixtures/composer/codex-empty.txt");
        let lines: Vec<&str> = codex.lines().collect();

        let claudes = ClaudeCode
            .composer_range(&lines)
            .expect("claude's locator finds two rules here, which is the trap");
        let codexs = Codex
            .composer_range(&lines)
            .expect("and the live composer is elsewhere");
        assert_ne!(claudes, codexs, "the two locators disagree, which is the whole point");

        // Reported as `claude`, so Claude's locator runs first — and its own recognition declines
        // the transcript it found, which is what sends the snapshot on rather than the region.
        assert_eq!(
            against_pair(
                Some("claude"),
                codex,
                Some(include_str!("../fixtures/composer/codex-empty.ansi.txt"))
            ),
            Composer::Empty
        );
    }

    /// A known kind whose pane rendered a marker it does not own fails open, not closed.
    ///
    /// Regression: resolving the marker from the reported kind proved nothing about what the pane
    /// actually rendered, and a failed strip used to keep the whole line — so a `claude` pane
    /// showing anything but `❯` reported `Occupied` and refused delivery on evidence the guard never
    /// had. A mismatch now means that harness cannot answer, and the probe tier decides.
    #[test]
    fn a_known_kind_that_does_not_match_its_own_marker_falls_through_to_the_probe() {
        // A codex box reported as claude: claude's harness cannot read it, codex's can, and the
        // answer comes from the one that recognized it rather than from the label.
        assert_eq!(
            against(
                Some("claude"),
                include_str!("../fixtures/composer/codex-bordered-empty.txt")
            ),
            Composer::Empty
        );
        // Nothing recognizes this one, so it fails open rather than being called occupied.
        assert_eq!(
            against(Some("claude"), include_str!("../fixtures/composer/unknown-marker.txt")),
            Composer::UnknownMarker
        );
    }

    #[test]
    fn a_labelled_top_border_still_counts_as_a_rule() {
        // Claude Code labels the top of its box (`──── repo ──`), so a rule is a line whose leading
        // run of `─` is either the whole trimmed line or at least three characters long.
        assert!(is_horizontal_rule("─────── repo ──"));
        assert!(is_horizontal_rule("──────────────"));
        assert!(is_horizontal_rule("  ────  "));
        assert!(!is_horizontal_rule("─ x"), "one dash then text is a bullet, not a rule");
        assert!(!is_horizontal_rule("❯ ─────"));
        assert!(!is_horizontal_rule(""));
        assert!(!is_horizontal_rule("   "));
    }
}
