//! What this tool knows about each supported agent CLI: whether it can be prompted right now, and how
//! it wants context handed to it at session start.
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

/// The hook event a brief answers.
const SESSION_START: &str = "SessionStart";

/// `context` in the `SessionStart` envelope: `{"hookSpecificOutput":{…,"additionalContext":…}}`.
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
    /// Eighty lines: a long paste can push a Codex marker past forty, and a composer taller than its
    /// pane cannot be read at all — see README "Known issues".
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
/// A second source, not a second format: herdr strips `detection`'s escapes whatever `--format` asks
/// for, so the styling exists only in the renderings it does not normalize.
const STYLED: Probe = Probe {
    source: "visible",
    format: "ansi",
    lines: 40,
};

/// A delivery margin sized past any pane, for a harness whose gap scales with the terminal instead
/// of being fixed furniture.
const PANE_SCALED_MARGIN: u32 = 500;

/// A coding-agent CLI whose composer this tool can read.
///
/// Not a registry of supported agents: a kind absent from [`HARNESSES`] still gets a check through
/// the probe tier in [`readiness`].
pub trait AgentHarness: std::fmt::Debug {
    /// herdr's own kind label for this harness, as `agent get` reports it.
    fn kind(&self) -> &'static str;

    /// The character this harness's composer input begins after.
    fn marker(&self) -> char;

    /// Where this harness draws its composer in a snapshot, as a range over `lines`.
    ///
    /// `None` when this harness cannot see a composer here, which the caller reads as "not this
    /// harness's, or not visible" and never as "empty".
    fn composer_range(&self, lines: &[&str]) -> Option<std::ops::Range<usize>>;

    /// `context` wrapped the way this harness's host wants it delivered at session start.
    ///
    /// Each impl's doc comment carries the evidence for what its host reads.
    ///
    /// # Errors
    ///
    /// [`serde_json::Error`], which two string fields cannot provoke.
    fn hook(&self, context: &str) -> Result<String, serde_json::Error>;

    /// The flags this harness's CLI expresses `model` and `effort` as.
    fn tuning(&self, model: Option<&str>, effort: Option<&str>) -> Vec<String>;

    /// What this harness needs read in order to answer.
    ///
    /// An override takes effect only when the kind is known: the probe tier reads with
    /// [`Probe::default`] before any harness is resolved.
    fn probe(&self) -> Probe {
        Probe::default()
    }

    /// How many rows this harness draws between a delivered message and the bottom of its pane.
    ///
    /// Defaults to [`PANE_SCALED_MARGIN`], the generous end; an impl overrides it only with a
    /// measured, smaller figure, and the measurement goes in its doc comment.
    fn delivery_margin(&self) -> u32 {
        PANE_SCALED_MARGIN
    }

    /// Whether this harness's composer holds unsubmitted input.
    ///
    /// `None` when the body is not recognizable as this harness's composer. The caller then fails
    /// open, so this must not guess.
    fn composer_occupied(&self, body: &[&str]) -> Option<bool> {
        occupied_after(self.marker(), body)
    }
}

/// Every harness this build can read, in probe order.
pub const HARNESSES: [&dyn AgentHarness; 2] = [&ClaudeCode, &Codex];

/// Resolves herdr's kind label back to the harness that reads it.
pub fn by_kind(kind: &str) -> Option<&'static dyn AgentHarness> {
    HARNESSES.into_iter().find(|harness| harness.kind() == kind)
}

/// Every kind a caller may name, for spelling the choices in a rejection.
pub fn kinds() -> Vec<&'static str> {
    HARNESSES.into_iter().map(AgentHarness::kind).collect()
}

/// How far back to read this kind's pane when looking for a message that was delivered to it.
///
/// `kind` is what `agent get` or `agent start` reported. An unknown kind gets the most generous
/// margin any harness declares: over-reading is free — herdr clamps the request to the pane — while
/// under-reading silently reports a delivered message as unproven.
pub fn delivery_margin(kind: Option<&str>) -> u32 {
    kind.and_then(by_kind)
        .map_or_else(widest_delivery_margin, AgentHarness::delivery_margin)
}

/// The most generous margin any harness in [`HARNESSES`] declares.
fn widest_delivery_margin() -> u32 {
    HARNESSES
        .into_iter()
        .map(AgentHarness::delivery_margin)
        .max()
        .unwrap_or(PANE_SCALED_MARGIN)
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
/// `kind` is what `agent get` reported, which selects the harness; `read` is handed the source,
/// format, and line count that harness asks for. Resolution is two-tiered: the reported kind's own
/// harness answers if it recognizes a composer, and anything else falls back to whichever harness
/// does.
///
/// **Fails open** when no composer could be located, or one was and no harness recognized it — both
/// deliver anyway, with a warning. The answer is a snapshot, not a lock: a human can start typing
/// between the read and the submission.
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
        // The generic two-rule search survives only here, to say which fail-open answer this is.
        return Ok(if prompt_box_range(&lines).is_some() {
            Composer::UnknownMarker
        } else {
            Composer::NoPromptBox
        });
    };

    Ok(match occupied {
        // A harness draws its own suggestions in the box, and the plain rendering shows them as
        // typed text — confirmed against the styled rendering before refusing.
        true if suggestion_only(harness, &read(STYLED.source, STYLED.format, STYLED.lines)?) => Composer::Empty,
        true => Composer::Occupied,
        false => Composer::Empty,
    })
}

/// One harness's whole answer: its own locator, then its own reading of what it located.
///
/// `None` if either half declines — the next harness then gets the whole snapshot, not the region.
fn recognize(harness: &'static dyn AgentHarness, lines: &[&str]) -> Option<(&'static dyn AgentHarness, bool)> {
    let range = harness.composer_range(lines)?;
    Some((harness, harness.composer_occupied(&lines[range])?))
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// Whether anything follows `marker` in a composer's body — the half every harness shares.
///
/// `None` when the body is not this marker's composer: no non-empty line, or a leading line that
/// does not begin with it. A marker that does not match means this harness cannot answer — never
/// that the line counts as text.
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
/// Faint — SGR 2 — is how a harness draws text it wrote for itself; faintness rather than styling
/// in general, because a draft can be coloured too. Answers `false` for a composer it cannot find,
/// so uncertainty keeps the guard. `harness` is the one that recognized the *plain* composer, so
/// both reads select the same region by the same rule.
fn suggestion_only(harness: &dyn AgentHarness, styled: &str) -> bool {
    let read: Vec<StyledLine> = styled.lines().map(StyledLine::read).collect();
    let plain: Vec<&str> = read.iter().map(|line| line.text.as_str()).collect();
    let Some(body) = harness.composer_range(&plain) else {
        return false;
    };

    let mut content = false;
    for line in &read[body] {
        for (character, faint) in line.text.chars().zip(&line.faint) {
            // The marker is furniture drawn unfaint, so counting it would answer `false` every time.
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
struct StyledLine {
    /// The line with its escape sequences removed.
    text: String,
    /// Whether the character at the same position was drawn faint.
    faint: Vec<bool>,
}

impl StyledLine {
    /// Splits a line into its text and its faintness, following the SGR sequences that set it.
    ///
    /// Every SGR sequence is parsed, not just faint's own: `0` and `22` — the resets — end a faint
    /// run, and a parser that skipped sequences it did not care about would miss them.
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
            // `ESC [ <parameters> <final byte>`; anything else is not a sequence this reads.
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
/// An empty parameter list is `0`, which is what `ESC[m` means. A colour's own arguments are
/// consumed rather than scanned: the `2` in `38;2;<r>;<g>;<b>` is the colour space, and reading it
/// as the faint attribute would call every coloured draft a suggestion.
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
/// answer from the other.
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

/// Whether a line is one of the box's borders: a leading run of `─` that is the whole trimmed line
/// or at least three characters long, so a labelled top border (`──── repo ──`) still counts.
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
    /// `styled` is absent only for a capture taken without one, where the plain text stands in.
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
            // The rules in this snapshot are transcript; the live composer sits below them.
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
            // Sixty typed lines fill the read and push the marker out of it.
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
            // The queued banner opens with a block marker, above the live composer.
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
            // The menu is drawn below the composer; the `/` itself is typed.
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
            // This harness collapses a paste into chips, so everything fits inside forty lines.
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
            // The queued notice is drawn faint inside the composer itself.
            name: "a queued-message notice drawn inside the composer is not a draft",
            kind: Some("claude"),
            plain: pair!("claude-working-queued").0,
            styled: pair!("claude-working-queued").1,
            expected: Composer::Empty,
        },
        Case {
            // Markers echoed into the transcript above the live composer.
            name: "markers echoed into a Claude Code transcript are not the composer",
            kind: Some("claude"),
            plain: pair!("claude-transcript").0,
            styled: pair!("claude-transcript").1,
            expected: Composer::Empty,
        },
        Case {
            // The menu is drawn above the composer here, where Codex draws its own below.
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
    /// Applied to every fixture read, so a probe raised or lowered changes what these tests see.
    fn tail(snapshot: &str, lines: u32) -> String {
        let all: Vec<&str> = snapshot.lines().collect();
        let start = all.len().saturating_sub(usize::try_from(lines).unwrap_or(usize::MAX));
        all[start..].join("\n")
    }

    /// `readiness` against one scenario's two files instead of a pane.
    ///
    /// Each rendering is served from its own capture: stripping escapes off the styled one would
    /// test a text herdr never produces.
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
    /// For the two captures that exist only as styled text: the plain tier is served the same
    /// snapshot with its escapes removed.
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

    /// One Codex version draws both borders around its composer and one draws neither.
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

    /// Both fixtures are the same sixty-line draft in the same pane; only the read depth differs.
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

        // The shorter file is the tail of the longer one.
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

    /// The snapshot is cut above the live marker, so the newest candidate left is an echo of an
    /// already-sent message with the agent's answer below it.
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

    #[test]
    fn the_read_is_the_one_the_resolved_harness_asked_for() {
        // A `Cell` because the read can be called more than once, so the closure must be `Fn`.
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

    #[test]
    fn a_failed_read_is_not_reported_as_a_composer_answer() {
        let answer = readiness(Some("claude"), |_source, _format, _lines| Err("the pane went away"));
        assert_eq!(answer, Err("the pane went away"));
    }

    #[test]
    fn both_fail_open_answers_carry_a_warning_and_neither_success_answer_does() {
        assert!(Composer::NoPromptBox.warning().is_some());
        assert!(Composer::UnknownMarker.warning().is_some());
        assert!(Composer::Empty.warning().is_none());
        assert!(Composer::Occupied.warning().is_none());
    }

    /// Claude's two-rule locator succeeds on a Codex snapshot — Codex draws full-width rules in its
    /// transcript — so taking the first locator to answer would never reach the live marker.
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

        // Reported as `claude`, so Claude's locator runs first and its own recognition declines.
        assert_eq!(
            against_pair(
                Some("claude"),
                codex,
                Some(include_str!("../fixtures/composer/codex-empty.ansi.txt"))
            ),
            Composer::Empty
        );
    }

    #[test]
    fn an_unrecognized_kind_is_read_as_far_back_as_the_most_generous_harness() {
        let unknown = delivery_margin(Some("some-agent-shipped-next-year"));

        for harness in HARNESSES {
            assert!(
                unknown >= harness.delivery_margin(),
                "{} is read further back than an unrecognized kind",
                harness.kind()
            );
        }
        assert_eq!(unknown, widest_delivery_margin());
        assert_eq!(delivery_margin(None), unknown, "a pane hosting no agent, likewise");
    }

    #[test]
    fn the_trait_default_is_the_most_generous_value_rather_than_a_middle_one() {
        #[derive(Debug)]
        struct Unmeasured;

        impl AgentHarness for Unmeasured {
            fn kind(&self) -> &'static str {
                "unmeasured"
            }
            fn marker(&self) -> char {
                '#'
            }
            fn composer_range(&self, _lines: &[&str]) -> Option<std::ops::Range<usize>> {
                None
            }
            fn hook(&self, context: &str) -> Result<String, serde_json::Error> {
                session_start(context)
            }
            fn tuning(&self, _model: Option<&str>, _effort: Option<&str>) -> Vec<String> {
                Vec::new()
            }
        }

        assert_eq!(Unmeasured.delivery_margin(), PANE_SCALED_MARGIN);
        assert!(Unmeasured.delivery_margin() >= widest_delivery_margin());
    }

    #[test]
    fn each_harness_answers_the_read_distance_its_own_rendering_calls_for() {
        let codex = delivery_margin(Some("codex"));
        let claude = delivery_margin(Some("claude"));

        assert!(
            claude >= codex * 4,
            "one harness pads down to the bottom of its pane and the other stops at its transcript; \
             a single number would over-read one and under-read the other"
        );
    }

    #[test]
    fn a_known_kind_that_does_not_match_its_own_marker_falls_through_to_the_probe() {
        // A codex box reported as claude: the answer comes from the harness that recognized it.
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
        assert!(is_horizontal_rule("─────── repo ──"));
        assert!(is_horizontal_rule("──────────────"));
        assert!(is_horizontal_rule("  ────  "));
        assert!(!is_horizontal_rule("─ x"), "one dash then text is a bullet, not a rule");
        assert!(!is_horizontal_rule("❯ ─────"));
        assert!(!is_horizontal_rule(""));
        assert!(!is_horizontal_rule("   "));
    }
}
