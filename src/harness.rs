//! What this tool knows about each supported agent CLI, which is one string per kind: its
//! composer's prompt marker.
//!
//! Touches no process and no pane — it is handed a detection snapshot as `&str` and answers whether
//! the composer holds text. The region rule lives here once; each kind supplies only its marker.

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// The composer prompt marker for each kind this build knows, keyed by herdr's own kind label.
///
/// Deliberately short. This is not a registry of supported agents — herdr's kind list has 21
/// entries and grows, and restating it here would drift. It is the per-harness half of one rule,
/// and a kind that is missing from it still gets a check through the probe tier below.
const MARKERS: [(&str, char); 2] = [("claude", '❯'), ("codex", '›')];

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

/// Whether the target's composer holds unsent text, read from a detection snapshot.
///
/// `kind` is what `agent get` reported, which selects the marker. Marker selection is two-tiered so
/// an unfamiliar kind still gets a check rather than none: a kind this build holds a marker for uses
/// it, and any other kind falls back to trying every known marker against the first non-empty body
/// line, taking the first that matches.
///
/// **Fails open** in exactly two cases — the body cannot be located, or no known marker matched.
/// Both deliver anyway, with a warning, because a tool that refused every pane it could not parse
/// would be unusable the first time a harness changed its rendering.
///
/// The guard is a snapshot, not a lock: a human can start typing between this read and the
/// submission. Narrowing that window further would need something herdr does not expose.
pub fn composer(kind: Option<&str>, snapshot: &str) -> Composer {
    let lines: Vec<&str> = snapshot.lines().collect();
    let Some(body) = prompt_box_body(&lines) else {
        return Composer::NoPromptBox;
    };
    // A body with no non-empty line at all is a box that is not a composer. Reported as an unknown
    // marker rather than as `Empty`, because no composer was identified and claiming one is empty
    // is a guarantee this did not earn.
    let Some(head) = body.iter().position(|line| !line.trim().is_empty()) else {
        return Composer::UnknownMarker;
    };
    let Some(marker) = marker_for(kind, body[head]) else {
        return Composer::UnknownMarker;
    };

    let leading = body[head].trim_start();
    let after_marker = leading.strip_prefix(marker).unwrap_or(leading);
    let occupied = !after_marker.trim().is_empty()
        || body
            .iter()
            .enumerate()
            .any(|(index, line)| index != head && !line.trim().is_empty());

    if occupied { Composer::Occupied } else { Composer::Empty }
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// The marker to strip: this kind's if the build knows it, otherwise the first known marker the
/// body's leading line actually starts with.
fn marker_for(kind: Option<&str>, head: &str) -> Option<char> {
    if let Some(kind) = kind
        && let Some((_, marker)) = MARKERS.iter().find(|(known, _)| *known == kind)
    {
        return Some(*marker);
    }
    MARKERS
        .iter()
        .find(|(_, marker)| head.trim_start().starts_with(*marker))
        .map(|(_, marker)| *marker)
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

    #[test]
    fn the_guard_answers_each_fixture_the_way_the_design_says() {
        for case in &CASES {
            assert_eq!(composer(case.kind, case.snapshot), case.expected, "{}", case.name);
        }
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
