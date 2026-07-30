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
    fn codexs_marker_is_not_this_harnesss_to_read() {
        // Answering `Some` here would let a mislabelled pane be judged by the wrong harness. `None`
        // sends it to the probe tier instead.
        assert_eq!(ClaudeCode.composer_occupied(&["› a codex draft"]), None);
    }
}
