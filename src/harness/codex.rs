//! Codex.

use super::AgentHarness;

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// The characters Codex opens a transcript block with.
///
/// Ported from herdr's own `codex_block_marker_line`, in `src/detect/manifest.rs`, which is where
/// they come from. Restated rather than derived, because nothing machine-readable publishes them —
/// named here so a future divergence has somewhere to be checked against.
const BLOCK_MARKERS: [char; 4] = ['•', '■', '✗', '✓'];

// =====================================================================================================================
// Codex
// =====================================================================================================================

/// Codex, herdr kind `codex`.
///
/// Its composer grows downward as a draft wraps, so a draft can sit on a line below the marker's.
/// The shared marker rule already counts any non-empty line in the body, which is what makes that
/// work without per-harness logic here.
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
    /// **Codex v0.146.0 draws no border around its composer — neither edge, not just the closing
    /// one — so the marker is the only thing there is to anchor on.** The full-width rules that do
    /// appear in a snapshot belong to its *transcript*, drawn after a tool call, and reading the
    /// region between the last two of them as the composer is the defect this replaces.
    ///
    /// **The marker is echoed too, so the last one is not always the live one.** Codex re-renders
    /// every submitted message behind the same `›`; one sixty-six-line snapshot held four marker
    /// lines and only the last was the composer. The discriminator is herdr's own, from
    /// `current_codex_prompt_index`: take the last marker, and reject it if any [`BLOCK_MARKERS`]
    /// line appears after it, because a marker with transcript below it *is* transcript. The scan
    /// then continues upward, which in practice ends it — a block marker below one candidate is
    /// below every earlier one — and herdr gives up at that point for the same reason.
    ///
    /// The body ends at the first blank line, which is what separates the composer from whatever
    /// Codex draws beneath it. Without that bound the body runs to the end of the snapshot and
    /// swallows it, and any non-empty line in a body counts as a draft — so an unbounded body would
    /// turn a guard that never fires into one that refuses every message. Referenced by position
    /// only: what sits below is not this build's to identify, and Codex rewrites it depending on
    /// what the composer holds. A rule ends the body too, for the older Codex that did draw one.
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
    /// Codex appends to its transcript and leaves the rest of the screen alone, so what sits below a
    /// delivered message is furniture and not a pane: the queued-message banner, the composer, and
    /// the footer. Measured against a sixty-five-row pane with a three-line message, the `<mail>`
    /// element sat about **ten rows** from the bottom — and a `--lines 100` read of that same pane
    /// answered forty-one rows, where Claude Code's answered sixty-four, which is the difference
    /// itself: one harness writes blank rows down to the bottom and this one stops.
    ///
    /// A dozen times the measurement, so the agent beginning to answer within the poll window cannot
    /// push the message out of the read. Still far below [`super::PANE_SCALED_MARGIN`], and the gap
    /// between the two numbers is the point: this one is bounded by what Codex draws, and that one
    /// is bounded by the terminal.
    fn delivery_margin(&self) -> u32 {
        120
    }

    /// The same `SessionStart` envelope Claude Code reads, on weaker evidence than that one.
    ///
    /// What is established: Codex runs a `SessionStart` hook, and its hooks answer with JSON on stdout
    /// — a generated Codex hook config falls back to `echo '{}'` on every event. What is *not*
    /// established is which keys it reads to inject context; that contract was looked for and not
    /// found. The shape here rests on the tool this borrowed the idea from wrapping Codex, Claude Code,
    /// and Gemini CLI in one envelope from a single flag.
    ///
    /// Written out rather than inherited so that reading Codex's real contract is an edit *here*,
    /// against a claim that says what it was based on.
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
/// Column zero rather than the first non-blank character, which is herdr's rule and not a
/// simplification of it: a draft may legitimately hold an indented bullet, and trimming first would
/// read that as transcript and lose the composer it sits inside.
fn block_marker_line(line: &str) -> bool {
    BLOCK_MARKERS.iter().any(|marker| line.starts_with(*marker))
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The composer is the marker line plus its continuations, bounded by the blank line below.
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

    /// Without that bound the body swallows the footer, and a footer is always non-empty — so every
    /// message to a Codex agent would be refused, naming a draft nobody wrote.
    #[test]
    fn an_untouched_composer_above_a_footer_is_empty_rather_than_occupied() {
        let lines = ["› ", "", "  gpt-5.6-codex medium · Context 95% left"];

        let range = Codex.composer_range(&lines).expect("the marker is right there");
        assert_eq!(Codex.composer_occupied(&lines[range]), Some(false));
    }

    /// A marker with a transcript block below it is a past message Codex echoed back.
    #[test]
    fn a_marker_followed_by_a_block_marker_is_transcript_rather_than_the_composer() {
        for block in BLOCK_MARKERS {
            let below = format!("{block} the agent's answer to it");
            let lines = ["› an already-sent message", "", below.as_str()];

            assert_eq!(Codex.composer_range(&lines), None, "{block}");
        }
    }

    /// The live composer is found first, so the echo above it is never reached.
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

    /// An indented bullet in a draft is someone's text, not Codex's transcript.
    ///
    /// The rejection matches at column zero for this reason, which is also where herdr matches it.
    #[test]
    fn a_bullet_inside_a_draft_does_not_disqualify_the_composer_it_sits_in() {
        let lines = ["› a list I am typing:", "  • the first item", "", "  Context 95% left"];

        assert_eq!(Codex.composer_range(&lines), Some(0..2));
    }

    /// The older Codex drew borders, and its closing one ends the body just as a blank line does.
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

    /// This harness stops at the end of its transcript, so its gap is furniture and stays small.
    ///
    /// Pinned by name, and well clear of the ten rows measured: the number is what decides whether a
    /// delivered message can be found, and getting it wrong has no symptom but a false "unproven".
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

    /// The reason this is a method rather than one shared spelling: Codex has no effort flag, so the
    /// same field reaches it as a config override.
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
