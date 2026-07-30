//! What every other module shares, and deliberately little.
//!
//! Three primitives and the output seam. This is the only module here with no dependency on the
//! others' vocabulary, which is the property that keeps it a leaf: `herdr` reads into these types
//! and writes from them, `harness` answers about a snapshot without naming any of them, and `cmd`
//! executes against all of it.
//!
//! There is no execution context and no target-resolution layer. herdr resolves agent targets
//! server-side — `agent prompt <target>` takes a pane id or a unique agent name and answers
//! `agent_not_found` or `agent_target_ambiguous` itself — so a context type would earn its keep
//! only by holding a read every command answers from, and there is none. Loading anything eagerly
//! would also be wrong: a malformed preset file would then break `prompt`, which never reads one.

#![allow(dead_code, reason = "the first callers land in Tasks 6 through 11")]

mod sink;

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
/// Deliberately unvalidated: herdr hands this out and takes it back, and nothing here ever parses
/// one. The newtype is not guarding against bad input, it prevents *our* mistake. Ids are read out
/// of differently shaped responses — `.result.pane.pane_id` from a split, `.result.root_pane.pane_id`
/// from a tab or a workspace — and handed to `agent start --pane`. Passing a tab id there is a
/// mix-up only this code can make, and this makes it a compile error.
///
/// `#[serde(transparent)]` is honest here for the same reason: with no validation to skip, the
/// wire form *is* the string.
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
/// The one validation this crate performs that herdr does not: herdr's prompt argument has no
/// non-empty constraint, so an empty prompt is at best a confusing refusal and at worst a bare
/// Enter delivered into a live agent.
///
/// Validated through `FromStr` rather than a clap `value_parser` so the same rule covers both
/// sources `MaybeStdin` resolves — an argv value and a stdin document — which a parser on the raw
/// argument cannot see.
#[derive(Clone, Debug, PartialEq, Eq, Display, AsRef, Deref)]
#[as_ref(forward)]
#[deref(forward)]
pub struct NonEmptyText(String);

impl FromStr for NonEmptyText {
    type Err = BlankTextError;

    /// Accepts the text only when it carries something to deliver.
    ///
    /// # Errors
    ///
    /// Returns [`BlankTextError`] when the text is empty or whitespace-only.
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
/// This is the stated exception to *validate only what herdr won't*: herdr refuses a bad name at
/// `agent start`, by which point the surface exists, and the precondition property — a rejected
/// command has changed nothing — is worth one restated rule. herdr stays the authority: the rule
/// lives in exactly this one place with a test pinning it, the message says it is herdr's, and
/// `invalid_agent_name` is still propagated if anything slips past.
///
/// The same argument would apply to the agent *kind* and is deliberately not acted on. herdr's kind
/// list lives only in its compile-time argument parser, its request schema types the field as a bare
/// string, and the number of kinds it recognizes demonstrably grows — hardcoding it is exactly the
/// drift the rule warns about.
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
    ///
    /// # Errors
    ///
    /// Returns [`InvalidAgentName`] when the value breaks any part of that rule. The parts are not
    /// distinguished, because herdr does not distinguish them either and a second wording would be
    /// a second authority.
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

/// herdr's own limit on an agent name, in bytes — its check is `name.len() <= 32`.
const MAX_AGENT_NAME_BYTES: usize = 32;

// =====================================================================================================================
// Errors
// =====================================================================================================================

/// Rejection of empty or whitespace-only prompt text.
///
/// Carries nothing: the text is the prompt, and a prompt may not reach an error message.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("prompt text must not be blank")]
pub struct BlankTextError;

/// Rejection of an agent name herdr would refuse.
///
/// The message names herdr as the authority, so a reader who disagrees with the rule knows which
/// project to take it up with.
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
        // herdr hands us the id and we hand it back; nothing here parses it. The newtype exists so
        // a tab id cannot be passed where a pane id belongs.
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
    fn blank_prompt_text_is_rejected() {
        // Nothing downstream catches this: herdr's prompt argument has no non-empty constraint, so
        // an empty prompt is a bare Enter delivered into a live agent.
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
        // This is the one pre-check that duplicates a herdr rule, because herdr's refusal would
        // arrive at `agent start` — after the surface exists. The message names herdr as the
        // authority, and these cases pin the rule read out of herdr's own validator.
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
