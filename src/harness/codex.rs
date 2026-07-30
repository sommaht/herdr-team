//! Codex.

use super::AgentHarness;

/// Codex, herdr kind `codex`.
///
/// Its composer grows downward as a draft wraps, so a draft can sit on a line below the marker's.
/// The shared rule already counts any non-empty line in the box, which is what makes that work
/// without per-harness logic here.
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
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
}
