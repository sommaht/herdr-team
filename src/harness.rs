//! What this tool knows about each supported agent CLI: whether it can be prompted right now.
//!
//! A caller asks [`readiness`] one question and learns nothing about how it was answered. That it
//! takes a terminal snapshot, which snapshot, and how much of one, are this module's business — a
//! command that knew those would have to be edited every time a harness needed something different
//! to look at.
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

// =====================================================================================================================
// Harness
// =====================================================================================================================

/// What a harness needs read from a target before it can judge readiness.
///
/// Two values that are meaningless apart, and the caller of [`readiness`] passes them through
/// without interpreting either — which is what keeps the choice of snapshot inside this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Probe {
    /// herdr's `--source`: which of a pane's several renderings to take.
    pub source: &'static str,
    /// How many lines of it are enough.
    pub lines: u32,
}

impl Default for Probe {
    /// The plain-text bottom-buffer snapshot herdr's own agent detection reads, which is where every
    /// harness known today renders its composer.
    ///
    /// Forty lines is more than any composer needs and costs nothing; only the bottom is used.
    fn default() -> Self {
        Self { source: "detection", lines: 40 }
    }
}

/// A coding-agent CLI whose composer this tool can read.
///
/// Deliberately not a registry of supported agents: herdr's kind list has 21 entries and grows, and
/// restating it here would drift. An entry buys one thing — the ability to read that harness's
/// composer — and a kind absent from it still gets a check through the probe tier in [`readiness`].
///
/// Both judgment methods have defaults, so an impl states only what makes it different. Today that
/// is one character; the deferred placeholder-vs-typed-text work overrides
/// [`composer_occupied`](Self::composer_occupied), and a harness that renders its composer somewhere
/// else overrides [`probe`](Self::probe).
pub trait AgentHarness {
    /// herdr's own kind label for this harness, as `agent get` reports it.
    fn kind(&self) -> &'static str;

    /// The character this harness's composer input begins after.
    fn marker(&self) -> char;

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
/// `None` for a kind this build does not know, which is the common case — the caller then probes.
fn by_kind(kind: &str) -> Option<&'static dyn AgentHarness> {
    HARNESSES.into_iter().find(|harness| harness.kind() == kind)
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
    read: impl FnOnce(&'static str, u32) -> Result<String, E>,
) -> Result<Composer, E> {
    let harness = kind.and_then(by_kind);
    let probe = harness.map_or_else(Probe::default, AgentHarness::probe);
    let snapshot = read(probe.source, probe.lines)?;

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

/// The lines between the last two horizontal rules, which is where every harness renders its
/// composer.
///
/// Harness-agnostic by construction, and the same region herdr's own agent detection extracts as
/// `prompt_box_body`.
fn prompt_box_body<'a>(lines: &'a [&'a str]) -> Option<&'a [&'a str]> {
    let mut rules = lines
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, line)| is_horizontal_rule(line))
        .map(|(index, _)| index);
    let bottom = rules.next()?;
    let top = rules.next()?;
    Some(&lines[top + 1..bottom])
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
        readiness(kind, |_source, _lines| {
            Ok::<_, std::convert::Infallible>(snapshot.to_owned())
        })
        .expect("reading a fixture is infallible")
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
        let mut asked = None;
        let _ = readiness(Some("claude"), |source, lines| {
            asked = Some((source, lines));
            Ok::<_, std::convert::Infallible>(String::new())
        });

        assert_eq!(asked, Some((Probe::default().source, Probe::default().lines)));
    }

    /// A read failure is the caller's, returned untouched rather than folded into an answer.
    #[test]
    fn a_failed_read_is_not_reported_as_a_composer_answer() {
        let answer = readiness(Some("claude"), |_source, _lines| Err("the pane went away"));
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
