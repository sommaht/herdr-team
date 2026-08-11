//! The four ways to make a pane for an agent to start in, and the one way to take it away again.

use clap::ValueEnum;
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use crate::core::{AgentName, PaneId};
use crate::herdr::{HerdrError, run, run_text};

// =====================================================================================================================
// Placement
// =====================================================================================================================

/// Where a new agent's pane comes from. `ValueEnum` so `spawn --placement` parses straight into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    /// Split the calling pane.
    Pane,
    /// A new tab, whose root pane the agent takes.
    Tab,
    /// A new workspace, whose root pane the agent takes.
    Workspace,
    /// A new Git worktree, whose workspace's root pane the agent takes.
    Worktree,
}

/// Whether a created surface takes the user's focus. An enum rather than a `bool`: this flag is
/// inverted relative to the CLI flag that sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// Focus the new surface — `--focus`, which is opt-in.
    Take,
    /// Leave the human where they were — `--no-focus`, the default.
    Leave,
}

impl Focus {
    /// The herdr flag this choice spells, stated in every argument vector.
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
/// `workspace` pins where the tab opens; `None` when the caller has no workspace of its own to name.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
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

/// Creates a Git worktree of `source`, labels its workspace `label`, and reports its root pane
/// along with the checkout it opened on.
///
/// `source` is a directory inside the repository to cut *from*, not where the checkout lands —
/// herdr chooses the path and always opens the root pane at the checkout root; reopening it deeper
/// is [`open_at`]'s job. `None` for `branch` or `base` leaves herdr's own defaults in force.
///
/// # Errors
///
/// Returns whatever [`run`] returned. `linked_worktree_source` is the refusal worth expecting:
/// herdr will not cut a worktree from inside a linked worktree.
pub fn create_worktree(
    label: &AgentName,
    source: &str,
    branch: Option<&str>,
    base: Option<&str>,
    focus: Focus,
) -> Result<(PaneId, Checkout), HerdrError> {
    let created: WorktreeCreated = run(&worktree_args(label, source, branch, base, focus))?;
    Ok((created.root_pane.pane_id, created.worktree))
}

/// The workspace a pane currently belongs to.
///
/// Asked rather than read from `HERDR_WORKSPACE_ID`: that variable is injected at pane creation and
/// silently goes stale when the pane is moved to another workspace.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
pub fn workspace_of(pane: &str) -> Result<String, HerdrError> {
    let state: PaneState = run(&get_args(pane))?;
    Ok(state.pane.workspace_id)
}

/// Runs one command line in a pane's shell, as if a person had typed it there.
///
/// This is what places a worktree spawn's agent in a subdirectory: `agent start` launches the agent
/// *through* the pane's shell, so the shell's directory at launch is the agent's directory. The one
/// place a path becomes shell syntax; see [`shell_quote`].
///
/// # Errors
///
/// Returns whatever [`run_text`] returned. A success does not mean the text landed: herdr presses
/// Enter without checking the shell has reached its prompt, so text sent too early is lost outright
/// and the caller confirms with [`foreground_cwd`].
pub fn open_at(pane: &PaneId, directory: &str) -> Result<(), HerdrError> {
    // `pane run` answers with an exit status and no body at all, so reading it as an envelope
    // fails on a `cd` that actually landed — hence [`run_text`].
    run_text(&run_args(pane, directory))?;
    Ok(())
}

/// The root of the repository herdr resolves for `cwd`, answered from any directory inside it.
///
/// # Errors
///
/// Returns whatever [`run`] returned. `not_git_worktree` is the refusal worth expecting: `cwd` is
/// not inside a repository at all.
pub fn repo_root(cwd: &str) -> Result<String, HerdrError> {
    let listed: WorktreeListed = run(&list_args(cwd))?;
    Ok(listed.source.repo_root)
}

/// Where a pane's shell actually is, as the pane itself reports it. `None` when herdr cannot read
/// it.
///
/// The success signal for [`open_at`], which cannot tell on its own whether the text it sent was
/// typed into a live prompt.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
pub fn foreground_cwd(pane: &PaneId) -> Result<Option<String>, HerdrError> {
    let state: PaneState = run(&get_args(pane))?;
    Ok(state.pane.foreground_cwd)
}

/// Closes a pane, taking whatever was running in it.
///
/// The target is a `&str` rather than a [`PaneId`]: it can be a string a caller typed, and whether
/// it names a pane at all is herdr's to answer.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
pub fn close(pane: &str) -> Result<(), HerdrError> {
    // herdr answers `{"type":"ok"}`; that the call happened is the whole result.
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

/// `tab create` and `workspace create`'s result; only the root pane is read.
#[derive(Debug, Deserialize)]
struct RootPaneCreated {
    root_pane: PaneRef,
}

/// The one field either response is read for.
#[derive(Debug, Deserialize)]
struct PaneRef {
    pane_id: PaneId,
}

/// `worktree create`'s result: the root pane, and the checkout the new workspace sits on.
#[derive(Debug, Deserialize)]
struct WorktreeCreated {
    root_pane: PaneRef,
    worktree: Checkout,
}

/// Where a worktree spawn landed, read from herdr's response and reported onward unchanged.
///
/// `branch` is optional because herdr's field is, not because a create is expected to produce a
/// detached checkout.
#[derive(Debug, Deserialize, Serialize)]
pub struct Checkout {
    /// The branch the worktree checked out — herdr's, whether generated or the one asked for.
    pub branch: Option<String>,
    /// The absolute path herdr put the checkout at.
    pub path: String,
}

/// `worktree list`'s result, read only for the repository its `--cwd` resolved to.
#[derive(Debug, Deserialize)]
struct WorktreeListed {
    source: WorktreeSource,
}

/// The one field a `worktree list` is made for here — `repo_root`, not `source_checkout_path`:
/// the two differ only inside a linked worktree, a source herdr refuses to cut from.
#[derive(Debug, Deserialize)]
struct WorktreeSource {
    repo_root: String,
}

/// `pane get`'s result, read for the two fields two different callers branch on.
#[derive(Debug, Deserialize)]
struct PaneState {
    pane: PaneFields,
}

/// The two fields a `pane get` is made for here.
#[derive(Debug, Deserialize)]
struct PaneFields {
    /// The workspace the pane currently sits in — see [`workspace_of`].
    workspace_id: String,
    /// Where the pane's shell actually is, absent when herdr cannot read it.
    ///
    /// `#[serde(default)]` because herdr omits the field rather than sending `null`.
    #[serde(default)]
    foreground_cwd: Option<String>,
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// `herdr pane split <PANE> --direction right --cwd <CWD> --(no-)focus`.
///
/// The anchor is `$HERDR_PANE_ID`, never herdr's `--current`, which resolves to whichever pane is
/// *focused* — not this one when the command runs from an unfocused pane.
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
/// `--workspace` pins the tab to the caller's workspace; omitted, herdr resolves to whichever one
/// is *UI-focused*, which is not the caller's whenever the human has clicked elsewhere.
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

/// `herdr worktree create --cwd <SOURCE> [--branch <NAME>] [--base <REF>] --label <LABEL>
/// --(no-)focus`.
///
/// `--cwd` names the directory to cut from; herdr derives the new surface's working directory.
fn worktree_args(label: &str, source: &str, branch: Option<&str>, base: Option<&str>, focus: Focus) -> Vec<String> {
    let mut args = ["worktree", "create", "--cwd", source].map(str::to_owned).to_vec();
    if let Some(branch) = branch {
        args.push("--branch".to_owned());
        args.push(branch.to_owned());
    }
    if let Some(base) = base {
        args.push("--base".to_owned());
        args.push(base.to_owned());
    }
    args.extend(["--label", label, focus.flag()].map(str::to_owned));
    args
}

/// `herdr pane get <PANE>`.
fn get_args(pane: &str) -> Vec<String> {
    ["pane", "get", pane].map(str::to_owned).to_vec()
}

/// `herdr pane run <PANE> "cd -- '<DIRECTORY>'"`.
///
/// One argv element: herdr joins everything after the pane id with a space before typing it.
fn run_args(pane: &str, directory: &str) -> Vec<String> {
    vec![
        "pane".to_owned(),
        "run".to_owned(),
        pane.to_owned(),
        format!("cd -- {}", shell_quote(directory)),
    ]
}

/// `herdr worktree list --cwd <CWD>`.
///
/// `--cwd` is any directory; herdr resolves the repository containing it.
fn list_args(cwd: &str) -> Vec<String> {
    ["worktree", "list", "--cwd", cwd].map(str::to_owned).to_vec()
}

/// A path as one shell word: single-quoted, with an embedded `'` spelled `'\''`; see the style
/// guide's External effects section.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
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
        // Without `--cwd` a split inherits the pane's original directory, not the shell's current one.
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
        assert_eq!(split_args("w4:p1", "/w", Focus::Take).last().unwrap(), "--focus");
        assert_eq!(tab_args(None, "reviewer", "/w", Focus::Take).last().unwrap(), "--focus");
        assert_eq!(workspace_args("reviewer", "/w", Focus::Take).last().unwrap(), "--focus");
        assert_eq!(
            worktree_args("reviewer", "/w", None, None, Focus::Take).last().unwrap(),
            "--focus"
        );
    }

    #[test]
    fn a_worktree_names_the_source_checkout_and_the_branch_and_base_when_it_has_them() {
        assert_eq!(
            worktree_args(
                "reviewer",
                "/work/repo",
                Some("worktree/fix-flake"),
                Some("origin/main"),
                Focus::Leave
            ),
            [
                "worktree",
                "create",
                "--cwd",
                "/work/repo",
                "--branch",
                "worktree/fix-flake",
                "--base",
                "origin/main",
                "--label",
                "reviewer",
                "--no-focus"
            ]
        );
    }

    #[test]
    fn a_worktree_omits_branch_and_base_rather_than_restating_herdrs_defaults() {
        assert_eq!(
            worktree_args("reviewer", "/work/repo", None, None, Focus::Leave),
            [
                "worktree",
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
    fn a_split_reads_its_pane_id_from_a_different_field_than_a_tab_or_a_workspace() {
        let split: PaneCreated =
            serde_json::from_str(r#"{"type":"pane_info","pane":{"pane_id":"w4:p18","tab_id":"w4:t1"}}"#).unwrap();
        assert_eq!(split.pane.pane_id, PaneId::from("w4:p18"));

        let tab: RootPaneCreated =
            serde_json::from_str(r#"{"type":"tab_created","tab":{"tab_id":"w4:t3"},"root_pane":{"pane_id":"w4:p17"}}"#)
                .unwrap();
        assert_eq!(tab.root_pane.pane_id, PaneId::from("w4:p17"));
    }

    #[test]
    fn a_worktree_is_read_for_its_root_pane_and_its_checkout_and_nothing_else() {
        let created: WorktreeCreated = serde_json::from_str(
            r#"{"type":"worktree_created","workspace":{"workspace_id":"w9"},"tab":{"tab_id":"w9:t1"},
                "root_pane":{"pane_id":"w9:p1"},
                "worktree":{"path":"/work/trees/repo/worktree-lucky-harbor-8e01",
                            "branch":"worktree/lucky-harbor-8e01","is_bare":false,
                            "is_detached":false,"is_prunable":false,"is_linked_worktree":true,
                            "label":"repo"}}"#,
        )
        .unwrap();

        assert_eq!(created.root_pane.pane_id, PaneId::from("w9:p1"));
        assert_eq!(created.worktree.branch.as_deref(), Some("worktree/lucky-harbor-8e01"));
        assert_eq!(created.worktree.path, "/work/trees/repo/worktree-lucky-harbor-8e01");
    }

    #[test]
    fn a_checkout_is_reported_as_the_two_fields_it_was_read_for() {
        let checkout = Checkout {
            branch: Some("worktree/lucky-harbor-8e01".to_owned()),
            path: "/work/trees/repo/worktree-lucky-harbor-8e01".to_owned(),
        };

        assert_eq!(
            serde_json::to_string(&checkout).unwrap(),
            r#"{"branch":"worktree/lucky-harbor-8e01","path":"/work/trees/repo/worktree-lucky-harbor-8e01"}"#
        );
    }

    #[test]
    fn a_workspace_lookup_asks_about_one_pane() {
        assert_eq!(get_args("w4:p17"), ["pane", "get", "w4:p17"]);
    }

    #[test]
    fn a_pane_get_reports_both_the_workspace_and_where_its_shell_actually_is() {
        let state: PaneState = serde_json::from_str(
            r#"{"pane":{"agent":"claude","agent_status":"working","cwd":"/work","focused":false,
            "foreground_cwd":"/work/repo/src","pane_id":"wJ:p1","revision":20,"tab_id":"wJ:t1",
            "workspace_id":"wJ"}}"#,
        )
        .unwrap();

        assert_eq!(state.pane.workspace_id, "wJ");
        assert_eq!(state.pane.foreground_cwd.as_deref(), Some("/work/repo/src"));
    }

    #[test]
    fn a_pane_whose_foreground_directory_herdr_cannot_read_answers_nothing_rather_than_failing() {
        let state: PaneState =
            serde_json::from_str(r#"{"pane":{"pane_id":"wJ:p1","tab_id":"wJ:t1","workspace_id":"wJ"}}"#).unwrap();

        assert_eq!(state.pane.workspace_id, "wJ");
        assert_eq!(state.pane.foreground_cwd, None);
    }

    #[test]
    fn a_close_names_the_pane_and_nothing_else() {
        assert_eq!(close_args("w4:p17"), ["pane", "close", "w4:p17"]);
    }

    #[test]
    fn a_repository_lookup_asks_about_one_directory() {
        assert_eq!(
            list_args("/work/repo/src/api"),
            ["worktree", "list", "--cwd", "/work/repo/src/api"]
        );
    }

    #[test]
    fn a_worktree_listing_is_read_for_the_repository_root_and_nothing_else() {
        let listed: WorktreeListed = serde_json::from_str(
            r#"{"type":"worktree_list",
            "source":{"repo_key":"repo","repo_name":"repo","repo_root":"/work/repo",
                      "source_checkout_path":"/work/repo","source_workspace_id":"w4"},
            "worktrees":[{"path":"/work/repo","branch":"main","is_bare":false,"is_detached":false,
                          "is_prunable":false,"is_linked_worktree":false,"label":"repo"}]}"#,
        )
        .unwrap();

        assert_eq!(listed.source.repo_root, "/work/repo");
    }

    #[test]
    fn a_directory_change_is_typed_into_the_pane_as_one_quoted_shell_word() {
        assert_eq!(
            run_args("w9:p1", "/work/trees/repo-8e01/src"),
            ["pane", "run", "w9:p1", "cd -- '/work/trees/repo-8e01/src'"]
        );
    }

    #[test]
    fn a_path_that_could_be_read_as_shell_syntax_is_inert_inside_its_quotes() {
        assert_eq!(shell_quote("/work/repo/src"), "'/work/repo/src'");
        assert_eq!(shell_quote("/work/don't/stop"), r"'/work/don'\''t/stop'");
        assert_eq!(shell_quote("/work/a; rm -rf ~"), "'/work/a; rm -rf ~'");
        assert_eq!(shell_quote("/work/$HOME/`id`"), "'/work/$HOME/`id`'");
        assert_eq!(shell_quote("--rf"), "'--rf'");

        assert_eq!(
            run_args("w9:p1", "-rf").last().unwrap(),
            "cd -- '-rf'",
            "a path opening with a dash is a path"
        );
    }

    #[test]
    fn a_pane_run_answers_with_no_body_at_all_which_is_why_it_is_not_read_as_one() {
        assert!(
            serde_json::from_str::<IgnoredAny>("").is_err(),
            "an empty answer is not an envelope, and reading it as one is the bug this pins"
        );
    }

    #[test]
    fn a_close_result_carries_nothing_worth_reading() {
        assert!(serde_json::from_str::<IgnoredAny>(r#"{"type":"ok"}"#).is_ok());
    }

    #[test]
    fn a_placement_serializes_as_the_flag_that_chose_it() {
        assert_eq!(serde_json::to_string(&Placement::Pane).unwrap(), r#""pane""#);
        assert_eq!(serde_json::to_string(&Placement::Tab).unwrap(), r#""tab""#);
        assert_eq!(serde_json::to_string(&Placement::Workspace).unwrap(), r#""workspace""#);
        assert_eq!(serde_json::to_string(&Placement::Worktree).unwrap(), r#""worktree""#);
    }
}
