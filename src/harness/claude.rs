//! Claude Code.

use super::AgentHarness;

/// Claude Code, herdr kind `claude`.
///
/// Its composer is a single input line inside a bordered box, with the marker in the gutter and any
/// draft to the right of it — so the shared marker rule reads it, and this impl adds nothing beyond
/// naming the marker.
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

    #[test]
    fn codexs_marker_is_not_this_harnesss_to_read() {
        // Answering `Some` here would let a mislabelled pane be judged by the wrong harness. `None`
        // sends it to the probe tier instead.
        assert_eq!(ClaudeCode.composer_occupied(&["› a codex draft"]), None);
    }
}
