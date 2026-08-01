//! Claude Code.

use super::AgentHarness;

/// Claude Code, herdr kind `claude`.
///
/// Its composer is a single input line inside a bordered box, with the marker in the gutter and any
/// draft to the right of it. Both borders are drawn and drawn reliably, which is what lets this
/// harness locate its composer by them and skip the marker scan Codex needs.
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
    /// Claude Code draws both borders of its composer and draws them reliably, so the rule search is
    /// exact here — and it stays within a forty-line window even under a sixty-line paste, because
    /// this harness collapses one into `[Pasted text #1 +13 lines]` chips.
    ///
    /// **Deliberately not the marker scan [`super::codex::Codex`] uses.** This harness echoes `❯`
    /// into its transcript too, but it has no block-marker vocabulary a stale one could be rejected
    /// with — so a scan would accept a transcript echo whenever the live composer sat past the
    /// window, and report someone's already-sent message as their unsent draft. It does not need
    /// one: it has borders, and they are what this reads.
    fn composer_range(&self, lines: &[&str]) -> Option<std::ops::Range<usize>> {
        super::prompt_box_range(lines)
    }

    /// Claude Code's documented `SessionStart` shape: a `hookSpecificOutput` object whose
    /// `additionalContext` string is injected into the session.
    ///
    /// This is the one host whose contract is the basis for [`super::session_start`] rather than an
    /// inference from it.
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

    /// Both borders drawn, so the region between them is exactly the composer.
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

    /// No borders, no answer — deliberately, and this is the case a marker scan would get wrong.
    ///
    /// The echo at the top is a message already sent. This harness has no block-marker vocabulary to
    /// reject one with, so a scan would read it as an unsent draft and refuse every message to the
    /// agent. Declining is the honest answer: the caller then fails open with a warning.
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
        // Answering `Some` here would let a mislabelled pane be judged by the wrong harness. `None`
        // sends it to the probe tier instead.
        assert_eq!(ClaudeCode.composer_occupied(&["› a codex draft"]), None);
    }
}
