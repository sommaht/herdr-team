//! Claude Code.

use super::AgentHarness;

/// Claude Code, herdr kind `claude`.
///
/// Its composer is a bordered box with the marker in the gutter; both borders are drawn reliably.
#[derive(Debug)]
pub struct ClaudeCode;

impl AgentHarness for ClaudeCode {
    fn kind(&self) -> &'static str {
        "claude"
    }

    /// U+276F, which Claude Code renders in its composer's gutter.
    fn marker(&self) -> char {
        '❯'
    }

    /// The lines between the snapshot's last two horizontal rules.
    ///
    /// No marker scan: `❯` is echoed into the transcript too, and this harness has no block-marker
    /// vocabulary a stale one could be rejected with — a scan would report an already-sent message
    /// as an unsent draft whenever the live composer sat past the window.
    fn composer_range(&self, lines: &[&str]) -> Option<std::ops::Range<usize>> {
        super::prompt_box_range(lines)
    }

    /// The default, and this is the harness the default is shaped around.
    ///
    /// Claude Code anchors its composer to the bottom of the screen and pads blank rows up to the
    /// transcript, so the gap scales with the pane — measured at about forty-five rows in a
    /// sixty-five-row pane. No fixed figure serves it; [`super::PANE_SCALED_MARGIN`] is sized past
    /// any pane, and herdr's clamp to the pane's height is the real bound.
    fn delivery_margin(&self) -> u32 {
        super::PANE_SCALED_MARGIN
    }

    /// Claude Code's documented `SessionStart` shape: a `hookSpecificOutput` object whose
    /// `additionalContext` string is injected into the session.
    ///
    /// The one host whose contract [`super::session_start`] is based on rather than inferred for.
    fn hook(&self, context: &str) -> Result<String, serde_json::Error> {
        super::session_start(context)
    }

    /// Both are plain flags: `--model opus --effort xhigh`.
    fn tuning(&self, model: Option<&str>, effort: Option<&str>) -> Vec<String> {
        let mut flags = Vec::new();
        if let Some(model) = model {
            flags.extend(["--model".to_owned(), model.to_owned()]);
        }
        if let Some(effort) = effort {
            flags.extend(["--effort".to_owned(), effort.to_owned()]);
        }
        flags
    }
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_box_is_the_marker_alone_and_a_draft_beside_it_is_text() {
        assert_eq!(ClaudeCode.composer_occupied(&["❯"]), Some(false));
        assert_eq!(ClaudeCode.composer_occupied(&["❯ "]), Some(false));
        assert_eq!(ClaudeCode.composer_occupied(&["❯ half a thought"]), Some(true));
    }

    #[test]
    fn the_delivery_margin_covers_a_whole_pane_rather_than_a_measured_gap() {
        const MEASURED_GAP: u32 = 45;
        const A_TALL_PANE: u32 = 200;

        assert_eq!(ClaudeCode.delivery_margin(), crate::harness::PANE_SCALED_MARGIN);
        assert!(ClaudeCode.delivery_margin() >= MEASURED_GAP * 4);
        assert!(
            ClaudeCode.delivery_margin() > A_TALL_PANE,
            "the gap grows with the terminal, so passing today's measurement is not enough"
        );
    }

    #[test]
    fn model_and_effort_are_both_plain_flags_on_this_cli() {
        assert_eq!(
            ClaudeCode.tuning(Some("opus"), Some("xhigh")),
            ["--model", "opus", "--effort", "xhigh"]
        );
    }

    #[test]
    fn either_half_stands_alone_and_neither_yields_nothing() {
        assert_eq!(ClaudeCode.tuning(Some("opus"), None), ["--model", "opus"]);
        assert_eq!(ClaudeCode.tuning(None, Some("xhigh")), ["--effort", "xhigh"]);
        assert!(ClaudeCode.tuning(None, None).is_empty());
    }

    #[test]
    fn the_composer_is_what_the_last_two_borders_enclose() {
        let lines = [
            "  ❯ an already-sent message",
            "",
            "─────────────────────────",
            "❯ still typing",
            "─────────────────────────",
            "  a status line",
        ];

        assert_eq!(ClaudeCode.composer_range(&lines), Some(3..4));
    }

    /// The echo at the top is a message already sent, which a marker scan would read as a draft.
    #[test]
    fn a_snapshot_without_both_borders_is_declined_rather_than_scanned_for_a_marker() {
        let lines = [
            "  ❯ an already-sent message",
            "",
            "❯ still typing",
            "",
            "  a status line",
        ];

        assert_eq!(ClaudeCode.composer_range(&lines), None);
    }

    #[test]
    fn codexs_marker_is_not_this_harnesss_to_read() {
        assert_eq!(ClaudeCode.composer_occupied(&["› a codex draft"]), None);
    }
}
