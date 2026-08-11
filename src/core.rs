//! Domain primitives, the retry schedule, and the output seam shared by every other module.

mod backoff;
mod sink;

pub use backoff::Backoff;
#[cfg(test)]
pub(crate) use sink::SharedBuf;
pub use sink::{OutputMode, Sink};

use std::borrow::Cow;
use std::str::FromStr;

use derive_more::{AsRef, Deref, Display, From};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// =====================================================================================================================
// Pane Ids
// =====================================================================================================================

/// A herdr pane id, exactly as herdr spelled it.
///
/// Unvalidated: the newtype exists so a tab or workspace id cannot be passed where a pane id
/// belongs.
#[derive(Clone, Debug, PartialEq, Eq, Hash, From, Display, AsRef, Deref, Serialize, Deserialize)]
#[from(forward)]
#[as_ref(forward)]
#[deref(forward)]
#[serde(transparent)]
pub struct PaneId(String);

/// Borrows the id where a `String` argument is being assembled.
impl<'a> From<&'a PaneId> for Cow<'a, str> {
    fn from(pane: &'a PaneId) -> Self {
        Cow::Borrowed(pane)
    }
}

// =====================================================================================================================
// Prompt Text
// =====================================================================================================================

/// Non-blank text delivered to an agent.
///
/// herdr does not check this, and an empty prompt is a bare Enter delivered into a live agent.
/// The check lives in `FromStr` so it covers both sources `MaybeStdin` resolves — argv and stdin.
#[derive(Clone, Debug, PartialEq, Eq, Display, AsRef, Deref)]
#[as_ref(forward)]
#[deref(forward)]
pub struct NonEmptyText(String);

impl NonEmptyText {
    /// Wraps text this crate composed rather than text a caller typed.
    pub fn composed(text: String) -> Self {
        Self(text)
    }
}

impl FromStr for NonEmptyText {
    type Err = BlankTextError;

    fn from_str(value: &str) -> Result<Self, BlankTextError> {
        if value.trim().is_empty() {
            Err(BlankTextError)
        } else {
            Ok(Self(value.to_owned()))
        }
    }
}

// =====================================================================================================================
// Agent Names
// =====================================================================================================================

/// An agent's name, pre-checked against the rule herdr will apply.
///
/// herdr refuses a bad name only at `agent start`, after the surface already exists; checking
/// here keeps a rejected command from changing anything.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Display, AsRef, Deref)]
#[as_ref(forward)]
#[deref(forward)]
pub struct AgentName(String);

/// Borrows the name where a `String` argument is being assembled.
impl<'a> From<&'a AgentName> for Cow<'a, str> {
    fn from(name: &'a AgentName) -> Self {
        Cow::Borrowed(name)
    }
}

impl FromStr for AgentName {
    type Err = InvalidAgentName;

    /// Applies herdr's rule: a leading lowercase letter, then lowercase letters, digits, `-`, or
    /// `_`, to at most 32 bytes.
    fn from_str(value: &str) -> Result<Self, InvalidAgentName> {
        let mut characters = value.chars();
        let starts_well = matches!(characters.next(), Some('a'..='z'));
        let rest_is_well_formed = characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || matches!(character, '-' | '_')
        });

        if starts_well && rest_is_well_formed && value.len() <= MAX_AGENT_NAME_BYTES {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidAgentName)
        }
    }
}

/// herdr's own limit on an agent name, in bytes.
const MAX_AGENT_NAME_BYTES: usize = 32;

// =====================================================================================================================
// Errors
// =====================================================================================================================

/// Rejection of empty or whitespace-only prompt text.
///
/// Carries no payload: the text is the prompt, and a prompt may not reach an error message.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("prompt text must not be blank")]
pub struct BlankTextError;

/// Rejection of an agent name herdr would refuse.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error(
    "herdr requires an agent name to start with a lowercase letter and hold only lowercase letters, \
     digits, '-' or '_' (1-32 characters)"
)]
pub struct InvalidAgentName;

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_id_is_carried_verbatim_in_both_directions() {
        let pane = PaneId::from("w4:p17");

        assert_eq!(pane.to_string(), "w4:p17");
        assert_eq!(&*pane, "w4:p17");
        assert_eq!(serde_json::to_string(&pane).unwrap(), r#""w4:p17""#);
        assert_eq!(serde_json::from_str::<PaneId>(r#""w4:p17""#).unwrap(), pane);
    }

    #[test]
    fn text_with_content_is_accepted_and_preserved() {
        for text in ["x", "  padded  ", "line one\nline two\n", "-", "--flag"] {
            assert_eq!(text.parse::<NonEmptyText>().unwrap().to_string(), text);
        }
    }

    #[test]
    fn composed_text_carries_a_structural_guarantee_rather_than_a_checked_one() {
        let text = NonEmptyText::composed("<mail from=\"worker\">\nhi\n</mail>".to_owned());
        assert_eq!(text.to_string(), "<mail from=\"worker\">\nhi\n</mail>");
    }

    #[test]
    fn blank_prompt_text_is_rejected() {
        for text in ["", " ", "\t", "\n\n", "  \n \t "] {
            assert_eq!(text.parse::<NonEmptyText>(), Err(BlankTextError));
        }
    }

    #[test]
    fn agent_names_herdr_accepts_are_accepted_here() {
        for value in ["a", "reviewer", "a1", "with-hyphen_and_9", "x".repeat(32).as_str()] {
            assert_eq!(value.parse::<AgentName>().unwrap().to_string(), value);
        }
    }

    #[test]
    fn agent_names_herdr_refuses_are_refused_here_with_herdrs_own_rule() {
        for value in ["", "1abc", "Reviewer", "review er", "review.er", "réviseur", "-lead"] {
            assert!(value.parse::<AgentName>().is_err(), "{value:?} must be refused");
        }
        assert!(
            "x".repeat(33).parse::<AgentName>().is_err(),
            "33 characters is over herdr's limit"
        );

        assert_eq!(
            "Reviewer".parse::<AgentName>().unwrap_err().to_string(),
            "herdr requires an agent name to start with a lowercase letter and hold only lowercase \
             letters, digits, '-' or '_' (1-32 characters)"
        );
    }
}
