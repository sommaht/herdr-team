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
//! Locating the box is harness-agnostic and lives here once, because it is the same rule herdr's own
//! detection applies. Everything inside it belongs to [`AgentHarness`] — today one character each,
//! behind default methods so a harness needing more overrides rather than special-cases.
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
    /// Forty lines is more than any composer needs and costs nothing; only the bottom is used.
    fn default() -> Self {
        Self {
            source: "detection",
            format: "text",
            lines: 40,
        }
    }
}

/// The rendering that still carries its escape sequences, read only to tell a suggestion from a draft.
///
/// A second source rather than a second format of the first: `detection` is handed back with its
/// escapes already stripped, whatever `--format` asks for, so the styling this needs exists only in
/// the renderings herdr does not normalize.
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
/// harness that renders its composer somewhere else overrides [`probe`](Self::probe). The three
/// *declarations* — kind, marker, and hook envelope — are required, because each is a claim about a
/// specific host that someone has to make deliberately.
pub trait AgentHarness: std::fmt::Debug {
    /// herdr's own kind label for this harness, as `agent get` reports it.
    fn kind(&self) -> &'static str;

    /// The character this harness's composer input begins after.
    fn marker(&self) -> char;

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
    /// The composer could not be located: fewer than two horizontal rules in the snapshot.
    NoPromptBox,
    /// A box was located but no known marker matched its first non-empty line.
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
/// it recognizes the box, and anything else — an unknown kind, or a known one whose pane rendered
/// something its harness cannot read — falls back to whichever harness does recognize it.
///
/// **Fails open** in exactly two cases — the box cannot be located, or no harness recognized it.
/// Both deliver anyway, with a warning, because a tool that refused every pane it could not parse
/// would be unusable the first time a harness changed its rendering.
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
    let Some(body) = prompt_box_body(&lines) else {
        return Ok(Composer::NoPromptBox);
    };

    // The reported kind's own harness first, then every other, so a kind this build does not know —
    // or a known one whose pane rendered something its harness cannot read — still gets a check.
    let occupied = harness.and_then(|harness| harness.composer_occupied(body)).or_else(|| {
        HARNESSES
            .into_iter()
            .find_map(|harness| harness.composer_occupied(body))
    });

    Ok(match occupied {
        // Content in the box is not yet a draft: a harness draws its own suggestions there, and the
        // plain rendering shows them exactly as it shows typed text. Confirmed against the styled
        // rendering before refusing, which costs a second read on the refusal path alone.
        Some(true) if suggestion_only(&read(STYLED.source, STYLED.format, probe.lines)?) => Composer::Empty,
        Some(true) => Composer::Occupied,
        Some(false) => Composer::Empty,
        // No harness recognized the box. Reported as an unknown marker rather than as `Empty`,
        // because claiming an unidentified composer is empty is a guarantee this did not earn.
        None => Composer::UnknownMarker,
    })
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
/// Answers `false` for a box it cannot find or one whose content is not wholly faint — the caller
/// then refuses, so every uncertainty here keeps the guard rather than dropping it.
fn suggestion_only(styled: &str) -> bool {
    let read: Vec<StyledLine> = styled.lines().map(StyledLine::read).collect();
    let plain: Vec<&str> = read.iter().map(|line| line.text.as_str()).collect();
    let Some(body) = prompt_box_range(&plain) else {
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

/// The lines between the last two horizontal rules, which is where every harness renders its
/// composer.
///
/// Harness-agnostic by construction, and the same region herdr's own agent detection extracts as
/// `prompt_box_body`.
fn prompt_box_body<'a>(lines: &'a [&'a str]) -> Option<&'a [&'a str]> {
    Some(&lines[prompt_box_range(lines)?])
}

/// Where that body sits, for a caller holding something else indexed the same way.
///
/// [`suggestion_only`] locates the box in the text and then asks about the styling beside it, which
/// needs the positions rather than the lines.
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

    /// One fixture, the kind `agent get` reported for it, and the answer it must produce.
    struct Case {
        name: &'static str,
        kind: Option<&'static str>,
        snapshot: &'static str,
        expected: Composer,
    }

    const CASES: [Case; 7] = [
        Case {
            name: "an idle Claude Code composer holds nothing",
            kind: Some("claude"),
            snapshot: include_str!("../fixtures/composer/claude-empty.txt"),
            expected: Composer::Empty,
        },
        Case {
            name: "a half-written line beside the marker is unsent text",
            kind: Some("claude"),
            snapshot: include_str!("../fixtures/composer/claude-occupied.txt"),
            expected: Composer::Occupied,
        },
        Case {
            name: "an idle Codex composer holds nothing",
            kind: Some("codex"),
            snapshot: include_str!("../fixtures/composer/codex-empty.txt"),
            expected: Composer::Empty,
        },
        Case {
            name: "a draft on a later line of the box counts too",
            kind: Some("codex"),
            snapshot: include_str!("../fixtures/composer/codex-multiline.txt"),
            expected: Composer::Occupied,
        },
        Case {
            // The second tier: an unfamiliar kind still gets a check rather than none, by trying
            // every known marker against the first non-empty body line.
            name: "an unknown kind falls back to probing every known marker",
            kind: Some("some-agent-this-build-has-never-heard-of"),
            snapshot: include_str!("../fixtures/composer/codex-empty.txt"),
            expected: Composer::Empty,
        },
        Case {
            name: "a snapshot with fewer than two rules fails open",
            kind: Some("claude"),
            snapshot: include_str!("../fixtures/composer/no-rules.txt"),
            expected: Composer::NoPromptBox,
        },
        Case {
            name: "a box whose marker matches nothing known fails open",
            kind: None,
            snapshot: include_str!("../fixtures/composer/unknown-marker.txt"),
            expected: Composer::UnknownMarker,
        },
    ];

    /// `readiness` against a fixture instead of a pane.
    ///
    /// The whole reason the read is a closure: this module never performs I/O, so a test hands it a
    /// literal and the read cannot fail.
    fn against(kind: Option<&str>, snapshot: &'static str) -> Composer {
        readiness(kind, |_source, _format, _lines| {
            Ok::<_, std::convert::Infallible>(snapshot.to_owned())
        })
        .expect("reading a fixture is infallible")
    }

    /// `readiness` against a *styled* fixture, standing in for both renderings herdr offers.
    ///
    /// The plain tier is served the same snapshot with its escapes removed, which is what herdr's
    /// own `detection` source hands back — the whole reason the guard could not see the difference.
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
        for case in &CASES {
            assert_eq!(against(case.kind, case.snapshot), case.expected, "{}", case.name);
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
            against(Some("claude"), include_str!("../fixtures/composer/codex-empty.txt")),
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
