//! Codex.

use super::AgentHarness;

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// The characters Codex opens a transcript block with.
///
/// Restated from herdr's own `codex_block_marker_line`; nothing machine-readable publishes them.
const BLOCK_MARKERS: [char; 4] = ['•', '■', '✗', '✓'];

// =====================================================================================================================
// Codex
// =====================================================================================================================

/// Codex, herdr kind `codex`.
///
/// Its composer grows downward as a draft wraps, so a draft can sit on a line below the marker's.
#[derive(Debug)]
pub struct Codex;

impl AgentHarness for Codex {
    fn kind(&self) -> &'static str {
        "codex"
    }

    /// U+203A, which Codex renders in its composer's gutter.
    fn marker(&self) -> char {
        '›'
    }

    /// The last marker line the transcript does not own, down to the first blank line, rule, or end.
    ///
    /// Codex v0.146.0 draws no border around its composer, and the full-width rules in a snapshot
    /// belong to its transcript — so the marker is the only anchor, and it is echoed behind every
    /// submitted message too. A candidate with a [`BLOCK_MARKERS`] line after it is transcript, not
    /// the composer. The blank-line bound keeps whatever Codex draws beneath the composer out; a
    /// rule ends the body too, for the older Codex that did draw one.
    ///
    /// No committed capture covers a draft that *opens* with a deliberate blank line, so that is not
    /// claimed either way.
    fn composer_range(&self, lines: &[&str]) -> Option<std::ops::Range<usize>> {
        let head = (0..lines.len()).rev().find(|&index| {
            lines[index].trim_start().starts_with(self.marker())
                && !lines[index + 1..].iter().copied().any(block_marker_line)
        })?;

        let end = lines[head + 1..]
            .iter()
            .position(|line| line.trim().is_empty() || super::is_horizontal_rule(line))
            .map_or(lines.len(), |offset| head + 1 + offset);

        Some(head..end)
    }

    /// Far less than the default, because this harness does not pad down to the bottom of its pane.
    ///
    /// Measured: in a sixty-five-row pane a delivered three-line message sat about ten rows from
    /// the bottom. A dozen times that, so an agent beginning to answer within the poll window
    /// cannot push the message out of the read.
    fn delivery_margin(&self) -> u32 {
        120
    }

    /// The same `SessionStart` envelope Claude Code reads, on weaker evidence than that one.
    ///
    /// Codex runs a `SessionStart` hook and its hooks answer JSON on stdout, but which keys it
    /// reads to inject context was looked for and not found.
    fn hook(&self, context: &str) -> Result<String, serde_json::Error> {
        super::session_start(context)
    }

    /// `--model` is a flag; effort is not, and reaches Codex as a config override —
    /// `-c model_reasoning_effort=xhigh`.
    fn tuning(&self, model: Option<&str>, effort: Option<&str>) -> Vec<String> {
        let mut flags = Vec::new();
        if let Some(model) = model {
            flags.extend(["--model".to_owned(), model.to_owned()]);
        }
        if let Some(effort) = effort {
            flags.extend(["-c".to_owned(), format!("model_reasoning_effort={effort}")]);
        }
        flags
    }
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// Whether a line opens a transcript block, which is what makes a marker above it a past message.
///
/// Matched at column zero, herdr's own rule: an indented bullet may be someone's draft.
fn block_marker_line(line: &str) -> bool {
    BLOCK_MARKERS.iter().any(|marker| line.starts_with(*marker))
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_composer_runs_from_the_marker_to_the_blank_line_that_separates_it_from_the_footer() {
        let lines = [
            "• Ran cargo test",
            "",
            "› first line of the draft",
            "  second line of the draft",
            "",
            "  gpt-5.6-codex medium · Context 95% left",
        ];

        assert_eq!(Codex.composer_range(&lines), Some(2..4), "the footer stays out");
    }

    #[test]
    fn an_untouched_composer_above_a_footer_is_empty_rather_than_occupied() {
        let lines = ["› ", "", "  gpt-5.6-codex medium · Context 95% left"];

        let range = Codex.composer_range(&lines).expect("the marker is right there");
        assert_eq!(Codex.composer_occupied(&lines[range]), Some(false));
    }

    #[test]
    fn a_marker_followed_by_a_block_marker_is_transcript_rather_than_the_composer() {
        for block in BLOCK_MARKERS {
            let below = format!("{block} the agent's answer to it");
            let lines = ["› an already-sent message", "", below.as_str()];

            assert_eq!(Codex.composer_range(&lines), None, "{block}");
        }
    }

    #[test]
    fn the_last_marker_wins_when_nothing_below_it_is_a_transcript_block() {
        let lines = [
            "› an already-sent message",
            "",
            "• the agent's answer to it",
            "",
            "› still typing",
            "",
            "  gpt-5.6-codex medium · Context 95% left",
        ];

        assert_eq!(Codex.composer_range(&lines), Some(4..5));
    }

    #[test]
    fn a_bullet_inside_a_draft_does_not_disqualify_the_composer_it_sits_in() {
        let lines = ["› a list I am typing:", "  • the first item", "", "  Context 95% left"];

        assert_eq!(Codex.composer_range(&lines), Some(0..2));
    }

    #[test]
    fn a_horizontal_rule_ends_the_body_for_the_codex_that_still_draws_one() {
        let lines = [
            "──────────────────────",
            "› first line of the draft",
            "  second line of the draft",
            "──────────────────────",
            "  ⌃C quit",
        ];

        assert_eq!(Codex.composer_range(&lines), Some(1..3));
    }

    #[test]
    fn a_snapshot_with_no_marker_at_all_is_not_this_harnesss_to_locate() {
        assert_eq!(Codex.composer_range(&["  line 60 of a paste", "", "  a footer"]), None);
        assert_eq!(Codex.composer_range(&[]), None);
    }

    #[test]
    fn an_empty_box_is_the_marker_alone_and_a_draft_beside_it_is_text() {
        assert_eq!(Codex.composer_occupied(&["›"]), Some(false));
        assert_eq!(Codex.composer_occupied(&["› still typing"]), Some(true));
    }

    #[test]
    fn a_draft_that_wrapped_onto_a_later_line_still_counts() {
        assert_eq!(
            Codex.composer_occupied(&["›", "  continued onto the next row"]),
            Some(true)
        );
    }

    #[test]
    fn claudes_marker_is_not_this_harnesss_to_read() {
        assert_eq!(Codex.composer_occupied(&["❯ a claude draft"]), None);
    }

    #[test]
    fn the_delivery_margin_clears_this_harnesss_measured_gap_without_covering_a_whole_pane() {
        const MEASURED_GAP: u32 = 10;

        assert_eq!(Codex.delivery_margin(), 120);
        assert!(Codex.delivery_margin() >= MEASURED_GAP * 4);
        assert!(
            Codex.delivery_margin() < crate::harness::PANE_SCALED_MARGIN,
            "this harness does not pad down to the bottom of its pane, and should not read as if it did"
        );
    }

    #[test]
    fn effort_reaches_codex_as_a_config_override_rather_than_a_flag() {
        assert_eq!(
            Codex.tuning(Some("gpt-5.6-sol"), Some("xhigh")),
            ["--model", "gpt-5.6-sol", "-c", "model_reasoning_effort=xhigh"]
        );
        assert_eq!(
            Codex.tuning(None, Some("medium")),
            ["-c", "model_reasoning_effort=medium"]
        );
        assert!(Codex.tuning(None, None).is_empty());
    }
}
