//! The three ways to make a pane for an agent to start in.

use serde::{Deserialize, Serialize};

use crate::core::{AgentName, PaneId};
use crate::herdr::{HerdrError, run};

// =====================================================================================================================
// Placement
// =====================================================================================================================

/// Where a new agent's pane comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
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
pub fn create_tab(label: &AgentName, cwd: &str, focus: Focus) -> Result<PaneId, HerdrError> {
    let created: RootPaneCreated = run(&tab_args(label, cwd, focus))?;
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

/// `herdr tab create --cwd <CWD> --label <LABEL> --(no-)focus`.
fn tab_args(label: &str, cwd: &str, focus: Focus) -> Vec<String> {
    ["tab", "create", "--cwd", cwd, "--label", label, focus.flag()]
        .map(str::to_owned)
        .to_vec()
}

/// `herdr workspace create --cwd <CWD> --label <LABEL> --(no-)focus`.
fn workspace_args(label: &str, cwd: &str, focus: Focus) -> Vec<String> {
    ["workspace", "create", "--cwd", cwd, "--label", label, focus.flag()]
        .map(str::to_owned)
        .to_vec()
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

    #[test]
    fn a_tab_and_a_workspace_are_labelled_with_the_agents_name() {
        assert_eq!(
            tab_args("reviewer", "/work/repo", Focus::Leave),
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
        assert_eq!(tab_args("reviewer", "/w", Focus::Take).last().unwrap(), "--focus");
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
    fn a_placement_serializes_as_the_flag_that_chose_it() {
        assert_eq!(serde_json::to_string(&Placement::Pane).unwrap(), r#""pane""#);
        assert_eq!(serde_json::to_string(&Placement::Tab).unwrap(), r#""tab""#);
        assert_eq!(serde_json::to_string(&Placement::Workspace).unwrap(), r#""workspace""#);
    }
}
