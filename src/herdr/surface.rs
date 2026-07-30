//! The three ways to make a pane for an agent to start in, and the one way to take it away again.

use clap::ValueEnum;
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use crate::core::{AgentName, PaneId};
use crate::herdr::{HerdrError, run};

// =====================================================================================================================
// Placement
// =====================================================================================================================

/// Where a new agent's pane comes from.
///
/// `ValueEnum` here is what makes `spawn --placement` parse straight into this type, so the parsed
/// shape *is* the domain type rather than a boolean triple translated into one. clap spells each
/// variant lowercase, which is what `Serialize` already emitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    /// Split the calling pane.
    Pane,
    /// A new tab, whose root pane the agent takes.
    Tab,
    /// A new workspace, whose root pane the agent takes.
    Workspace,
}

/// Whether a created surface takes the user's focus.
///
/// An enum rather than a `bool`, because a bare `true` at a call site says nothing about which way
/// it points, and this flag is inverted relative to the CLI flag that sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// Focus the new surface — `--focus`, which is opt-in.
    Take,
    /// Leave the human where they were — `--no-focus`, the default.
    Leave,
}

impl Focus {
    /// The herdr flag this choice spells. Stated in every argument vector rather than relying on
    /// herdr's default, so the vector says what it means.
    fn flag(self) -> &'static str {
        match self {
            Self::Take => "--focus",
            Self::Leave => "--no-focus",
        }
    }
}

// =====================================================================================================================
// Surfaces
// =====================================================================================================================

/// Splits `pane` and reports the pane that appeared.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
pub fn split(pane: &PaneId, cwd: &str, focus: Focus) -> Result<PaneId, HerdrError> {
    let created: PaneCreated = run(&split_args(pane, cwd, focus))?;
    Ok(created.pane.pane_id)
}

/// Creates a tab labelled `label` and reports its root pane.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
/// `workspace` pins where the tab opens; see [`tab_args`] for why it is not left to herdr's
/// default. `None` when the caller has no workspace of its own to name.
pub fn create_tab(workspace: Option<&str>, label: &AgentName, cwd: &str, focus: Focus) -> Result<PaneId, HerdrError> {
    let created: RootPaneCreated = run(&tab_args(workspace, label, cwd, focus))?;
    Ok(created.root_pane.pane_id)
}

/// Creates a workspace labelled `label` and reports its root pane.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
pub fn create_workspace(label: &AgentName, cwd: &str, focus: Focus) -> Result<PaneId, HerdrError> {
    let created: RootPaneCreated = run(&workspace_args(label, cwd, focus))?;
    Ok(created.root_pane.pane_id)
}

/// Closes a pane, taking whatever was running in it.
///
/// The target is a `&str` rather than a [`PaneId`], unlike the three above: theirs come from the
/// environment or from a herdr response, while this one can be a string a caller typed. Whether it
/// names a pane at all is herdr's to answer, and passing it through unexamined is how this crate
/// stays out of guessing at an id format.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
pub fn close(pane: &str) -> Result<(), HerdrError> {
    // herdr answers `{"type":"ok"}`. That the call happened is the whole result.
    run::<IgnoredAny>(&close_args(pane))?;
    Ok(())
}

// =====================================================================================================================
// Responses
// =====================================================================================================================

/// `pane split`'s result: the pane it made.
#[derive(Debug, Deserialize)]
struct PaneCreated {
    pane: PaneRef,
}

/// `tab create` and `workspace create`'s result. Both also report the tab, and the workspace form
/// reports the workspace; neither is read here, and serde ignores what it is not asked for.
#[derive(Debug, Deserialize)]
struct RootPaneCreated {
    root_pane: PaneRef,
}

/// The one field either response is read for.
#[derive(Debug, Deserialize)]
struct PaneRef {
    pane_id: PaneId,
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// `herdr pane split <PANE> --direction right --cwd <CWD> --(no-)focus`.
///
/// The anchor is passed as `$HERDR_PANE_ID`, never as herdr's `--current`: that flag resolves
/// server-side to whichever pane is *focused*, which is not this one when the command runs from an
/// unfocused pane.
fn split_args(pane: &str, cwd: &str, focus: Focus) -> Vec<String> {
    [
        "pane",
        "split",
        pane,
        "--direction",
        "right",
        "--cwd",
        cwd,
        focus.flag(),
    ]
    .map(str::to_owned)
    .to_vec()
}

/// `herdr tab create [--workspace <ID>] --cwd <CWD> --label <LABEL> --(no-)focus`.
///
/// `--workspace` is passed whenever the caller's own is known, for the same reason `split` names
/// `$HERDR_PANE_ID` rather than herdr's `--current`: omitting it makes herdr resolve the workspace
/// server-side to whichever one is *UI-focused*, which is not the caller's whenever the human has
/// clicked elsewhere. A tab meant for this project then opens in whatever the user was last
/// looking at. Absent the variable there is nothing to pin, and herdr's own default stands.
fn tab_args(workspace: Option<&str>, label: &str, cwd: &str, focus: Focus) -> Vec<String> {
    let mut args = vec!["tab".to_owned(), "create".to_owned()];
    if let Some(workspace) = workspace {
        args.push("--workspace".to_owned());
        args.push(workspace.to_owned());
    }
    args.extend(["--cwd", cwd, "--label", label, focus.flag()].map(str::to_owned));
    args
}

/// `herdr workspace create --cwd <CWD> --label <LABEL> --(no-)focus`.
fn workspace_args(label: &str, cwd: &str, focus: Focus) -> Vec<String> {
    ["workspace", "create", "--cwd", cwd, "--label", label, focus.flag()]
        .map(str::to_owned)
        .to_vec()
}

/// `herdr pane close <PANE>`.
fn close_args(pane: &str) -> Vec<String> {
    ["pane", "close", pane].map(str::to_owned).to_vec()
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_split_names_the_anchor_pane_and_always_passes_a_cwd() {
        // The pane id is positional, which herdr accepts as the first argument that does not start
        // with `--`. `--cwd` is passed in all three modes: a split would otherwise inherit the
        // *pane's* working directory, which stops matching the shell's the moment you `cd`.
        assert_eq!(
            split_args("w4:p1", "/work/repo", Focus::Leave),
            [
                "pane",
                "split",
                "w4:p1",
                "--direction",
                "right",
                "--cwd",
                "/work/repo",
                "--no-focus"
            ]
        );
    }

    /// A tab is pinned to the caller's workspace, never left to herdr's focus-dependent default.
    ///
    /// Regression: without `--workspace`, herdr resolves the workspace server-side to whichever one
    /// the UI has focused. A rehearsal launch opened its tab in an unrelated project's workspace
    /// because the human had clicked there — the same trap `split` avoids by naming
    /// `$HERDR_PANE_ID` instead of `--current`.
    #[test]
    fn a_tab_names_the_callers_own_workspace_when_it_has_one() {
        assert_eq!(
            tab_args(Some("w7"), "reviewer", "/work/repo", Focus::Leave),
            [
                "tab",
                "create",
                "--workspace",
                "w7",
                "--cwd",
                "/work/repo",
                "--label",
                "reviewer",
                "--no-focus"
            ]
        );
    }

    #[test]
    fn a_tab_and_a_workspace_are_labelled_with_the_agents_name() {
        assert_eq!(
            tab_args(None, "reviewer", "/work/repo", Focus::Leave),
            [
                "tab",
                "create",
                "--cwd",
                "/work/repo",
                "--label",
                "reviewer",
                "--no-focus"
            ]
        );
        assert_eq!(
            workspace_args("reviewer", "/work/repo", Focus::Leave),
            [
                "workspace",
                "create",
                "--cwd",
                "/work/repo",
                "--label",
                "reviewer",
                "--no-focus"
            ]
        );
    }

    #[test]
    fn focus_is_always_stated_rather_than_left_to_herdrs_default() {
        // Opt-in: a tool meant to be driven by agents should not steal the human's focus. Stated
        // explicitly in both directions so the argument vector says what it means.
        assert_eq!(split_args("w4:p1", "/w", Focus::Take).last().unwrap(), "--focus");
        assert_eq!(tab_args(None, "reviewer", "/w", Focus::Take).last().unwrap(), "--focus");
        assert_eq!(workspace_args("reviewer", "/w", Focus::Take).last().unwrap(), "--focus");
    }

    #[test]
    fn a_split_reads_its_pane_id_from_a_different_field_than_a_tab_or_a_workspace() {
        // The reason `PaneId` exists: `.result.pane.pane_id` from a split, `.result.root_pane.pane_id`
        // from a tab or a workspace. Both feed `agent start --pane`, and mixing them up is a
        // mistake only this code can make.
        let split: PaneCreated =
            serde_json::from_str(r#"{"type":"pane_info","pane":{"pane_id":"w4:p18","tab_id":"w4:t1"}}"#).unwrap();
        assert_eq!(split.pane.pane_id, PaneId::from("w4:p18"));

        let tab: RootPaneCreated =
            serde_json::from_str(r#"{"type":"tab_created","tab":{"tab_id":"w4:t3"},"root_pane":{"pane_id":"w4:p17"}}"#)
                .unwrap();
        assert_eq!(tab.root_pane.pane_id, PaneId::from("w4:p17"));
    }

    #[test]
    fn a_close_names_the_pane_and_nothing_else() {
        // No flags exist on herdr's side: `pane close` takes a pane id and acts. Every guard in front
        // of it is this crate's.
        assert_eq!(close_args("w4:p17"), ["pane", "close", "w4:p17"]);
    }

    #[test]
    fn a_close_result_carries_nothing_worth_reading() {
        // herdr answers `{"type":"ok"}`, which is why `close` deserializes into `IgnoredAny` rather
        // than minting a response struct with no fields anyone reads.
        assert!(serde_json::from_str::<IgnoredAny>(r#"{"type":"ok"}"#).is_ok());
    }

    #[test]
    fn a_placement_serializes_as_the_flag_that_chose_it() {
        assert_eq!(serde_json::to_string(&Placement::Pane).unwrap(), r#""pane""#);
        assert_eq!(serde_json::to_string(&Placement::Tab).unwrap(), r#""tab""#);
        assert_eq!(serde_json::to_string(&Placement::Workspace).unwrap(), r#""workspace""#);
    }
}
