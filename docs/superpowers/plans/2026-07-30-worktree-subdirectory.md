# Worktree Subdirectory Placement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `--placement worktree` open the agent at the same relative path inside the new checkout that the caller was working in, and give `--cwd` one meaning across all four placements — *where the agent works*.

**Architecture:** Two files. `src/herdr/surface.rs` gains three calls — `worktree list` for the repository root, `pane run` for a quoted `cd`, and `pane get` for `foreground_cwd` — plus the shell-quoting rule, which lives beside the call that needs it. `src/cmd/spawn.rs` gains a read-only pre-step that computes the caller's path relative to its repository root, and a placement step that runs between `create_surface` and `agent start`: it checks the directory exists in the fresh checkout, sends the `cd`, and retries on `Backoff::within(--settle-timeout)` until `foreground_cwd` matches. Both failures degrade to the checkout root with a warning rather than refusing, because by the time either is knowable the checkout already exists.

**Tech Stack:** Rust 2024, clap derive, `thiserror`, `serde`, `tempfile` (dev). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-30-worktree-subdirectory-design.md`

---

## Conventions this codebase enforces

Read these before Task 1; every task assumes them.

- **Section headers.** Copy the separator verbatim from an adjacent file. Heavyweight for a major section, and the standard skeleton is `Constants`, one header per domain group, `Helpers`, `Tests`:

```rust
// =====================================================================================================================
// Section Name
// =====================================================================================================================
```

- **Tests are inline** `#[cfg(test)] mod tests` under a `Tests` header. No integration-test binary. **Automated tests never invoke herdr** — build what you need through the same constructors the read path uses (`serde_json::from_str` for a response struct, `.parse()` for a domain type). Every new function that decides something is split so its decision is a pure function taking values, and *that* is what the tests exercise.
- **Argument vectors are pinned by exact-string tests, one per herdr call.** Three new calls arrive here, so three new vector tests.
- **Wire forms are pinned by exact-string tests.** The shell quoting is a wire form: it is what another program parses.
- **No prompt text, preset arguments, or captured terminal content may reach an error message, a diagnostic, or a log.** Nothing here handles prompt text, but the two new warnings are still held to the shape the rule produces: they name the directory that was missed and carry nothing else.
- **Nothing outside the `herdr` module spawns a process or names the `herdr` binary.** That is why the repository root is read with `herdr worktree list` rather than with `git`.
- **`--cwd` is a caller-supplied path and the callers are agents.** It becomes shell syntax exactly once, in `run_args`, and the quoting is tested over a hostile path.
- **Preconditions run before anything in herdr changes.** The repository-root read is read-only and happens before `create_surface`. Everything after `create_surface` leaves the pane open and names it.
- Test function names are full sentences describing the behaviour, in the style already present in each file.
- `rustfmt.toml` sets `max_width = 120`. The style gate already prompts on `src/cmd/spawn.rs` for file length (RS-033); this plan grows it further. That prompt is a candidate, not a finding — it does not fail the run, and the file's own module doc already argues why the spawn transaction stays one file.

## File structure

| File | Change | Responsibility |
| --- | --- | --- |
| `docs/STYLE-GUIDE.md` | Modify | Names `pane run` as the one place shell text may exist |
| `src/herdr/surface.rs` | Modify | `worktree list`, `pane run`, `pane get`'s `foreground_cwd`, and the quoting rule |
| `src/cmd/spawn.rs` | Modify | The relative path, the placement step, and the two warnings |

---

## Task 1: Name the one place shell text may exist

The spec requires a path to become shell syntax. The style guide currently forbids that outright, in
two places, with the words "there is no exception". Amend it first, so the rest of this plan is not a
violation of a rule that was never revisited.

There is no test here, deliberately: the guide is prose read by people and by agents, and the thing it
constrains is pinned by the exact-string tests in Task 2.

**Files:**
- Modify: `docs/STYLE-GUIDE.md` (the `External effects` section, and one `Forbidden` bullet)

- [ ] **Step 1: Read the two passages you are about to change**

Run: `grep -n "interpolat" docs/STYLE-GUIDE.md`
Expected: two hits — one in `## External effects`, one in `## Forbidden`.

- [ ] **Step 2: Rewrite the `External effects` section**

Replace the whole section (from `## External effects` to the blank line before `## Tests`) with:

```markdown
## External effects

Every process invocation is built from `Command` args — never an interpolated shell string. herdr
takes the agent's arguments as an argument vector, so nothing this crate *runs* needs shell
quoting.

A preset's `args` is therefore an **array only**. A string form would have to be split into shell
words, which means reimplementing shell word-splitting for a value handed to `Command`.

**The one exception is `herdr pane run`**, whose argument is a command line herdr types into a
pane's shell. The invocation is still an argument vector — what becomes syntax is the value
*inside* one of its arguments, and that value is caller-supplied. So the quoting rule lives in
`herdr::surface` beside the call that needs it, in one function, and is pinned by exact-string
tests over a hostile path: the path is single-quoted with an embedded `'` spelled `'\''`, and `cd`
is given `--` so a path opening with a dash cannot be read as a flag. It is a wire form — it is
what another program parses. Nowhere else builds shell text, and a second site is a design
decision with a document, not a call someone adds.
```

- [ ] **Step 3: Amend the `Forbidden` bullet**

The bullet currently reads:

```markdown
- Shell-string interpolation anywhere; every process invocation is built from `Command` args.
```

Change it to:

```markdown
- Shell-string interpolation outside the one quoting function in `herdr::surface`; every process
  invocation is built from `Command` args.
```

- [ ] **Step 4: Verify the guide no longer contradicts the spec**

Run: `grep -n "no exception\|pane run" docs/STYLE-GUIDE.md`
Expected: the phrase "There is no exception" is gone, and `pane run` is named twice.

- [ ] **Step 5: Commit**

```bash
git add docs/STYLE-GUIDE.md
git commit -m "docs(style): name the one place shell text is allowed to exist"
```

---

## Task 2: The quoting rule and `pane run`

The pure half first. No herdr process is involved in anything this task tests.

**Files:**
- Modify: `src/herdr/surface.rs`

- [ ] **Step 1: Write the failing tests**

Add to `src/herdr/surface.rs`'s `mod tests`:

```rust
#[test]
fn a_directory_change_is_typed_into_the_pane_as_one_quoted_shell_word() {
    // The command line is one argv element: herdr joins everything after the pane id with a
    // space before typing it, so splitting it here would only be rejoined.
    assert_eq!(
        run_args("w9:p1", "/work/trees/repo-8e01/src"),
        ["pane", "run", "w9:p1", "cd -- '/work/trees/repo-8e01/src'"]
    );
}

/// The quoting is a wire form: it is what another program parses, so it is pinned by exact string.
///
/// `--cwd` is caller-supplied and the callers are agents, so the hostile cases are the point. Inside
/// single quotes a shell expands nothing, which makes `$HOME`, a backtick and a `;` literal
/// characters; the one thing a single-quoted string cannot hold is a single quote, which is why one
/// is spelled `'\''`. `cd --` is what keeps a leading dash a path rather than a flag.
#[test]
fn a_path_that_could_be_read_as_shell_syntax_is_inert_inside_its_quotes() {
    assert_eq!(shell_quote("/work/repo/src"), "'/work/repo/src'");
    assert_eq!(shell_quote("/work/don't/stop"), r"'/work/don'\''t/stop'");
    assert_eq!(shell_quote("/work/a; rm -rf ~"), "'/work/a; rm -rf ~'");
    assert_eq!(shell_quote("/work/$HOME/`id`"), "'/work/$HOME/`id`'");
    assert_eq!(shell_quote("--rf"), "'--rf'");

    // The separator is what covers the leading dash, since the quotes alone do not.
    assert_eq!(
        run_args("w9:p1", "-rf").last().unwrap(),
        "cd -- '-rf'",
        "a path opening with a dash is a path"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-targets surface`
Expected: FAIL — `cannot find function run_args`, `cannot find function shell_quote`.

- [ ] **Step 3: Write the implementation**

In `src/herdr/surface.rs`, add to the `Surfaces` section, below `workspace_of`:

```rust
/// Runs one command line in a pane's shell, as if a person had typed it there.
///
/// This is what places a worktree spawn's agent in a subdirectory: `agent start` launches the agent
/// *through* the pane's shell — it appears in the pane as a typed command line — so the shell's
/// directory at launch is the agent's directory. One call, and no pane is split, moved, or closed.
///
/// The one place in this crate where a path stops being an argument and becomes syntax, which is why
/// the quoting lives here beside the call: see [`shell_quote`] and the `External effects` section of
/// the style guide.
///
/// # Errors
///
/// Returns whatever [`run`] returned. Note what a success does *not* mean: herdr types the text and
/// presses Enter without checking that the shell has reached its prompt, so text sent too early is
/// lost outright. The caller confirms the outcome with [`foreground_cwd`] rather than assuming this
/// landed.
pub fn open_at(pane: &PaneId, directory: &str) -> Result<(), HerdrError> {
    // herdr answers `{"type":"ok"}`. That the text was sent is the whole result.
    run::<IgnoredAny>(&run_args(pane, directory))?;
    Ok(())
}
```

In the `Helpers` section, below `get_args`:

```rust
/// `herdr pane run <PANE> "cd -- '<DIRECTORY>'"`.
///
/// The command line is one argv element because herdr joins everything after the pane id with a
/// space before typing it — three elements here would arrive as the same one string, and building it
/// as one is what makes the vector say what herdr will see.
fn run_args(pane: &str, directory: &str) -> Vec<String> {
    vec![
        "pane".to_owned(),
        "run".to_owned(),
        pane.to_owned(),
        format!("cd -- {}", shell_quote(directory)),
    ]
}

/// A path as one shell word: single-quoted, with an embedded `'` closed, escaped, and reopened.
///
/// Single quotes are the strong form — inside them a shell expands nothing, so `$HOME`, a backtick
/// and a `;` are literal characters and a path holding a command separator is inert rather than
/// executed. The one thing a single-quoted string cannot hold is a single quote, which is why one is
/// spelled `'\''`: close, escape, reopen.
///
/// Quoting alone does not cover a path that opens with a dash, which a shell would read as a flag.
/// [`run_args`] covers that by giving `cd` a `--` separator.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --all-targets surface`
Expected: PASS, including every pre-existing `surface` test.

- [ ] **Step 5: Commit**

```bash
git add src/herdr/surface.rs
git commit -m "feat(herdr): type a quoted directory change into a pane's shell"
```

---

## Task 3: The repository a directory belongs to

`worktree list --cwd <path>` resolves from any directory inside a repository, which is what makes the
relative path computable from one read-only call before anything is created. No `git` process is
involved, which is what keeps the rule that nothing outside this module spawns one.

**Files:**
- Modify: `src/herdr/surface.rs`

- [ ] **Step 1: Write the failing tests**

Add to `src/herdr/surface.rs`'s `mod tests`:

```rust
#[test]
fn a_repository_lookup_asks_about_one_directory() {
    // Any directory inside the repository answers, which is the property that makes this callable
    // from wherever the caller happens to be standing.
    assert_eq!(
        list_args("/work/repo/src/api"),
        ["worktree", "list", "--cwd", "/work/repo/src/api"]
    );
}

/// A listing is read for one string, and everything else herdr says about the repository is dropped.
///
/// `source_checkout_path` sits beside `repo_root` and is deliberately not the field read. The two
/// differ only when the caller is already inside a linked worktree, and herdr refuses to cut a
/// worktree from there — see [`create_worktree`].
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-targets surface`
Expected: FAIL — `cannot find function list_args`, `cannot find type WorktreeListed`.

- [ ] **Step 3: Write the implementation**

In `src/herdr/surface.rs`, add to the `Surfaces` section, below `open_at`:

```rust
/// The root of the repository herdr resolves for `cwd`, answered from any directory inside it.
///
/// Read before a worktree is cut, so `spawn` can work out where the caller sits inside its
/// repository and reopen the new checkout's pane at the same place. One read-only call, and no `git`
/// process — which is what keeps the rule that nothing outside this module spawns one.
///
/// # Errors
///
/// Returns whatever [`run`] returned. `not_git_worktree` is the refusal worth expecting: `cwd` is
/// not inside a repository at all, which `worktree create` would refuse a moment later anyway — and
/// meeting it here is better, because nothing has been created yet.
pub fn repo_root(cwd: &str) -> Result<String, HerdrError> {
    let listed: WorktreeListed = run(&list_args(cwd))?;
    Ok(listed.source.repo_root)
}
```

In the `Responses` section, below `Checkout`:

```rust
/// `worktree list`'s result, read only for the repository its `--cwd` resolved to.
///
/// herdr also reports every worktree of that repository; serde drops what it is not asked for, and
/// this call is made for one string.
#[derive(Debug, Deserialize)]
struct WorktreeListed {
    source: WorktreeSource,
}

/// The one field a `worktree list` is made for here.
///
/// herdr's source object also carries `repo_key`, `repo_name`, `source_checkout_path` and
/// `source_workspace_id`. `source_checkout_path` is the one worth naming as *not* read: it differs
/// from `repo_root` only when the caller is inside a linked worktree, which is a source herdr
/// refuses to cut from.
#[derive(Debug, Deserialize)]
struct WorktreeSource {
    repo_root: String,
}
```

In the `Helpers` section, below `run_args`:

```rust
/// `herdr worktree list --cwd <CWD>`.
///
/// `--cwd` is a directory rather than a repository root: herdr resolves the repository containing
/// it, which is the whole reason this answers the question `spawn` has.
fn list_args(cwd: &str) -> Vec<String> {
    ["worktree", "list", "--cwd", cwd].map(str::to_owned).to_vec()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --all-targets surface`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/herdr/surface.rs
git commit -m "feat(herdr): read the repository a directory belongs to"
```

---

## Task 4: Where a pane's shell actually is

The success signal for the placement step. `pane get` already has one caller here — `workspace_of` —
so this reads a second field off the same response rather than minting a second response type for the
same herdr command.

**Files:**
- Modify: `src/herdr/surface.rs`

- [ ] **Step 1: Write the failing tests**

In `src/herdr/surface.rs`'s `mod tests`, **replace** the existing test
`a_pane_get_is_read_only_for_the_workspace_and_ignores_the_rest` with the two below. Keep its doc
comment on the first one — the regression it records is still the reason `workspace_of` exists.

```rust
/// The workspace comes from herdr, never from the pane's environment.
///
/// Regression, and the reason this call exists at all: `HERDR_WORKSPACE_ID` is injected when a
/// pane is created and cannot be rewritten in a running shell afterwards. A pane moved to another
/// workspace keeps the old id, so a tab spawned from it would open in the workspace it *used* to
/// be in — verified live, where `pane get` reported the new workspace after a move while the
/// variable could not have.
///
/// The same response answers a second question now: `foreground_cwd` is where the pane's shell
/// actually is, which is how the worktree placement step learns whether its `cd` landed.
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
    // herdr omits the field rather than sending null, and `workspace_of` predates it entirely — so
    // a response without it has to parse, or the older caller breaks on the newer field.
    let state: PaneState =
        serde_json::from_str(r#"{"pane":{"pane_id":"wJ:p1","tab_id":"wJ:t1","workspace_id":"wJ"}}"#).unwrap();

    assert_eq!(state.pane.workspace_id, "wJ");
    assert_eq!(state.pane.foreground_cwd, None);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-targets surface`
Expected: FAIL — `cannot find type PaneState`.

- [ ] **Step 3: Write the implementation**

In `src/herdr/surface.rs`, add to the `Surfaces` section, below `repo_root`:

```rust
/// Where a pane's shell actually is, as the pane itself reports it. `None` when herdr cannot read
/// it.
///
/// Asked rather than matched against the shell's own output: the pane's report of where it is beats
/// text a shell chose to print, which changes with the shell. This is the success signal for
/// [`open_at`], which cannot tell on its own whether the text it sent was typed into a live prompt
/// or into a shell that had not started yet.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
pub fn foreground_cwd(pane: &PaneId) -> Result<Option<String>, HerdrError> {
    let state: PaneState = run(&get_args(pane))?;
    Ok(state.pane.foreground_cwd)
}
```

In the `Responses` section, **replace** `PaneWorkspace` and `WorkspaceRef` with:

```rust
/// `pane get`'s result, read for the two fields two different callers branch on.
///
/// A separate type from [`PaneCreated`] rather than optional fields on [`PaneRef`]: a creation
/// response does not report a workspace at all, and one struct spanning both would make the field
/// optional in the one place it is required. One type for both readers of *this* command, though —
/// two structs over one response would be two places to revise when herdr adds a field either wants.
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
    /// `#[serde(default)]` because herdr omits the field rather than sending `null`, and because
    /// [`workspace_of`] predates it and must keep parsing a response that has none.
    #[serde(default)]
    foreground_cwd: Option<String>,
}
```

Update `workspace_of`'s body to name the new type:

```rust
pub fn workspace_of(pane: &str) -> Result<String, HerdrError> {
    let state: PaneState = run(&get_args(pane))?;
    Ok(state.pane.workspace_id)
}
```

Extend `get_args`'s doc comment, which now serves two callers:

```rust
/// `herdr pane get <PANE>`.
///
/// One call with two readers: [`workspace_of`] wants the workspace, [`foreground_cwd`] wants the
/// shell's directory.
fn get_args(pane: &str) -> Vec<String> {
    ["pane", "get", pane].map(str::to_owned).to_vec()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --all-targets surface`
Expected: PASS. `a_workspace_lookup_asks_about_one_pane` is untouched — the type was renamed, the
call was not.

- [ ] **Step 5: Commit**

```bash
git add src/herdr/surface.rs
git commit -m "feat(herdr): report where a pane's shell actually is"
```

---

## Task 5: Where the caller sits inside its repository

A pure function over two strings. `Path::strip_prefix` rather than `str::strip_prefix`, because it
compares components: it must not match `/repo` against `/repository/src`, and it has to survive a
trailing separator on either side.

**Files:**
- Modify: `src/cmd/spawn.rs`

- [ ] **Step 1: Write the failing tests**

Add to `src/cmd/spawn.rs`'s `mod tests`:

```rust
#[test]
fn a_caller_inside_a_project_is_placed_at_the_same_path_in_the_new_checkout() {
    assert_eq!(relative_to("/work/repo", "/work/repo/src").as_deref(), Some("src"));
    assert_eq!(
        relative_to("/work/repo", "/work/repo/src/api/handlers").as_deref(),
        Some("src/api/handlers")
    );
}

#[test]
fn a_caller_at_the_repository_root_has_nothing_to_place_and_skips_the_step() {
    // Not an empty string that a later `cd` would run as a no-op: the pane already opens at the
    // checkout root, so there is nothing for the placement step to do.
    assert_eq!(relative_to("/work/repo", "/work/repo"), None);
}

#[test]
fn a_trailing_separator_on_either_side_is_not_part_of_the_relative_path() {
    assert_eq!(relative_to("/work/repo/", "/work/repo/src").as_deref(), Some("src"));
    assert_eq!(relative_to("/work/repo", "/work/repo/src/").as_deref(), Some("src"));
    assert_eq!(relative_to("/work/repo/", "/work/repo/"), None);
}

/// The reason this compares path components rather than string prefixes.
///
/// `str::strip_prefix` would answer `"y/src"` here and send the agent somewhere that does not exist.
/// The second case is the caller already inside a linked worktree, whose `repo_root` is the main
/// checkout it is not under — herdr refuses to cut a worktree from there, and answering `None`
/// keeps this from computing nonsense in the moment before that refusal arrives.
#[test]
fn a_directory_that_only_looks_like_a_prefix_of_the_root_is_not_inside_it() {
    assert_eq!(relative_to("/work/repo", "/work/repository/src"), None);
    assert_eq!(relative_to("/work/repo", "/work/trees/repo-8e01/src"), None);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-targets spawn`
Expected: FAIL — `cannot find function relative_to`.

- [ ] **Step 3: Write the implementation**

In `src/cmd/spawn.rs`, add to the `Helpers` section, below `anchor`:

```rust
/// Where `cwd` sits inside `repo_root`, or `None` when there is nothing to place.
///
/// `None` covers two cases that want the same answer: a caller already at the repository root, which
/// has no subdirectory to reopen at, and a `cwd` that is not inside `repo_root` at all.
///
/// [`Path::strip_prefix`] rather than [`str::strip_prefix`], because it compares components. The
/// string form answers `"y/src"` for `/repo` against `/repository/src`, and it would have to be
/// taught about a trailing separator on either side, which the path form already knows.
fn relative_to(repo_root: &str, cwd: &str) -> Option<String> {
    let relative = Path::new(cwd).strip_prefix(repo_root).ok()?;
    if relative.as_os_str().is_empty() {
        None
    } else {
        Some(relative.to_string_lossy().into_owned())
    }
}
```

Extend the imports at the top of the file — `PathBuf` is already imported, `Path` is not:

```rust
use std::path::{Path, PathBuf};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --all-targets spawn`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/cmd/spawn.rs
git commit -m "feat(spawn): work out where the caller sits inside its repository"
```

---

## Task 6: The directory to open at, and what to say when it cannot

A directory in the caller's checkout can be absent from a fresh one — ignored, untracked, or absent
from the ref `--base` names. That is a filesystem question about a local checkout, so it is one
`is_dir` rather than a process, a herdr call, or a read of the shell's own error text.

Both ways of missing the subdirectory produce the same outcome — start at the checkout root and say
which directory was missed — so they get one owner and one wording rule.

**Files:**
- Modify: `src/cmd/spawn.rs`

- [ ] **Step 1: Write the failing tests**

Add to `src/cmd/spawn.rs`'s `mod tests`:

```rust
#[test]
fn a_subdirectory_the_fresh_checkout_does_not_have_falls_back_to_the_checkout_root() {
    // Absent because it is gitignored, untracked, or not in the ref `--base` named. `None` *is* the
    // checkout root: it skips the placement step, and the pane herdr made already opens there.
    let checkout = tempfile::tempdir().unwrap();

    assert_eq!(target_in(&checkout.path().to_string_lossy(), "src/api"), None);
}

#[test]
fn a_subdirectory_the_fresh_checkout_does_have_is_where_the_pane_is_sent() {
    let checkout = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(checkout.path().join("src/api")).unwrap();

    assert_eq!(
        target_in(&checkout.path().to_string_lossy(), "src/api"),
        Some(checkout.path().join("src/api"))
    );

    // A file of that name is not a directory to `cd` into either.
    std::fs::write(checkout.path().join("README.md"), "x").unwrap();
    assert_eq!(target_in(&checkout.path().to_string_lossy(), "README.md"), None);
}

/// Both warnings name the directory that was missed, and neither carries anything else.
///
/// The checkout path is deliberately absent: the result line already reports it, and a warning that
/// repeats it is noise on the one line a caller reads. What a caller cannot get anywhere else is
/// which directory it asked for and did not get.
#[test]
fn both_warnings_name_the_directory_that_was_missed_and_carry_nothing_else() {
    assert_eq!(
        Unplaced::Absent.warning("src/api"),
        "src/api is not in the new checkout, so the agent starts at its root"
    );
    assert_eq!(
        Unplaced::NeverArrived.warning("src/api"),
        "the new pane never reached src/api, so the agent starts at the checkout root"
    );

    for warning in [Unplaced::Absent, Unplaced::NeverArrived] {
        let rendered = warning.warning("src/api");
        assert!(rendered.contains("src/api"), "{rendered}");
        assert!(
            !rendered.contains("/work/trees"),
            "the checkout path is the result line's to report, not a warning's"
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-targets spawn`
Expected: FAIL — `cannot find function target_in`, `cannot find type Unplaced`.

- [ ] **Step 3: Write the implementation**

In `src/cmd/spawn.rs`, add a section above `Output`:

```rust
// =====================================================================================================================
// Placement
// =====================================================================================================================

/// Why an agent could not be started in the subdirectory the caller asked for.
///
/// Neither is a failure. By the time either is knowable the checkout exists, and this crate does not
/// tear down what it created — the same reason a failure after a surface exists leaves the pane open
/// and names it. A worktree deleted to report a directory that was gitignored anyway is a worse
/// answer than one that works from the root and says so.
///
/// One type for both because they share a wording rule: name the directory that was missed and carry
/// nothing else. The checkout path is the result line's to report.
#[derive(Clone, Copy, Debug)]
enum Unplaced {
    /// The directory is not in the fresh checkout: gitignored, untracked, or absent from `--base`.
    Absent,
    /// The pane's shell never reached the directory inside the settle window.
    NeverArrived,
}

impl Unplaced {
    /// What the caller is told.
    fn warning(self, directory: &str) -> String {
        match self {
            Self::Absent => format!("{directory} is not in the new checkout, so the agent starts at its root"),
            Self::NeverArrived => {
                format!("the new pane never reached {directory}, so the agent starts at the checkout root")
            }
        }
    }
}
```

Add to the `Helpers` section, below `relative_to`:

```rust
/// The directory in a fresh checkout to open at, or `None` when the checkout does not have it.
///
/// One `is_dir` rather than a process, a herdr call, or a read of the shell's own error text: the
/// checkout is local and so is this tool. Answered before anything is typed into the pane, which is
/// what keeps the two failures apart — the retry loop that follows only ever waits for a shell.
fn target_in(checkout: &str, relative: &str) -> Option<PathBuf> {
    let target = Path::new(checkout).join(relative);
    target.is_dir().then_some(target)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --all-targets spawn`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/cmd/spawn.rs
git commit -m "feat(spawn): decide the directory to open at, and what to say when it cannot"
```

---

## Task 7: The placement step

Everything above assembled into the flow: one read-only call before anything is created, then the
`cd` between `create_surface` and `agent start`.

**Files:**
- Modify: `src/cmd/spawn.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/cmd/spawn.rs`'s `mod tests`:

```rust
/// Three of the four placements answer without asking herdr anything.
///
/// That is what makes this testable at all — no automated test in this crate invokes herdr, so a
/// version that called out for every placement would have nothing to assert here. The worktree arm
/// is the one this cannot cover, and it is covered by the live rehearsal instead.
#[test]
fn only_a_worktree_spawn_asks_where_the_caller_sits_in_its_repository() {
    for placement in ["pane", "tab", "workspace"] {
        let args = parse(&["spawn", "reviewer", "--placement", placement]);

        assert_eq!(args.subdirectory("/work/repo/src").unwrap(), None, "{placement}");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --all-targets spawn`
Expected: FAIL — `no method named subdirectory`.

- [ ] **Step 3: Write the implementation**

In `src/cmd/spawn.rs`, add to the first `impl SpawnArgs` block, below `workspace`:

```rust
    /// The caller's own directory relative to its repository root, for a worktree spawn to reopen
    /// at.
    ///
    /// One read-only herdr call, made before anything is created, and only for the placement that
    /// has a second checkout to map into. `None` from a caller already at the repository root, which
    /// skips the placement step rather than running a no-op `cd`.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Herdr`] when herdr cannot resolve a repository for `cwd` — `not_git_worktree`
    /// for a caller outside one. That refusal now arrives here rather than from `worktree create`,
    /// which is where a precondition belongs: nothing has been created yet, so a rejected command
    /// has changed nothing.
    fn subdirectory(&self, cwd: &str) -> Result<Option<String>, SpawnError> {
        if self.placement != Placement::Worktree {
            return Ok(None);
        }
        Ok(relative_to(&surface::repo_root(cwd)?, cwd))
    }
```

Add to the second `impl SpawnArgs` block, below `start_when_settled`:

```rust
    /// Opens the new checkout's pane at the caller's own subdirectory, before the agent starts in
    /// it.
    ///
    /// `agent start` launches the agent *through* the pane's shell, so the shell's directory at
    /// launch is the agent's directory — which is what makes one [`surface::open_at`] enough, with
    /// no pane split, moved, or closed.
    ///
    /// Never a refusal. Both ways of missing the subdirectory warn and return `Ok`, for the reason
    /// [`Unplaced`] records.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Herdr`] if herdr refused the `pane run` or the `pane get` outright — a
    /// transport failure rather than a shell that has not caught up.
    fn open_subdirectory(&self, pane: &PaneId, checkout: &str, relative: &str, sink: &Sink) -> Result<(), SpawnError> {
        let Some(target) = target_in(checkout, relative) else {
            sink.warn(&Unplaced::Absent.warning(relative));
            return Ok(());
        };
        let target = target.to_string_lossy().into_owned();

        // The same budget and schedule `agent start` retries on, and for the same cause: a shell
        // running a directory-environment hook is not at its prompt yet, and `pane run` types into
        // it regardless — so text sent too early is lost outright rather than queued. Re-sent each
        // round rather than polled, because a lost `cd` is never going to arrive on its own.
        for delay in Backoff::within(self.settle_timeout) {
            if arrived(pane, &target)? {
                return Ok(());
            }
            std::thread::sleep(delay);
        }

        // One last attempt after the budget is spent, so a zero settle timeout still tries once.
        if !arrived(pane, &target)? {
            sink.warn(&Unplaced::NeverArrived.warning(relative));
        }
        Ok(())
    }
```

Add to the `Helpers` section, below `target_in`:

```rust
/// Sends the directory change, then asks the pane where its shell actually ended up.
///
/// Testing the outcome rather than assuming the command landed is the whole point: `pane run`
/// succeeds whether the text reached a live prompt or a shell that had not started, and only
/// [`surface::foreground_cwd`] can tell those apart.
///
/// # Errors
///
/// Whatever either herdr call returned.
fn arrived(pane: &PaneId, target: &str) -> Result<bool, HerdrError> {
    surface::open_at(pane, target)?;
    Ok(surface::foreground_cwd(pane)?.as_deref() == Some(target))
}
```

Now wire it into `execute`. The current body reads:

```rust
        let cwd = cwd.to_string_lossy().into_owned();

        // Everything above is read-only, so nothing exists yet if any of it failed.
        let (pane, worktree) = self.create_surface(anchor, &cwd, sink)?;

        // From here on a failure leaves the pane open and names it: whatever went wrong is on
        // screen in it, and closing it would throw the error away with it.
        self.start_and_prompt(&pane, &kind, &agent_args, worktree, sink)
            .map_err(|error| error.note_open_pane(&pane))
```

Change it to:

```rust
        let cwd = cwd.to_string_lossy().into_owned();

        // Read before anything is created, so a caller outside a repository is refused while a
        // refusal is still free.
        let subdirectory = self.subdirectory(&cwd)?;

        // Everything above is read-only, so nothing exists yet if any of it failed.
        let (pane, worktree) = self.create_surface(anchor, &cwd, sink)?;

        // From here on a failure leaves the pane open and names it: whatever went wrong is on
        // screen in it, and closing it would throw the error away with it.
        //
        // Before the agent, because `agent start` inherits the shell's directory — after it, the
        // shell would move and the agent would not.
        if let (Some(checkout), Some(relative)) = (&worktree, &subdirectory) {
            self.open_subdirectory(&pane, &checkout.path, relative, sink)
                .map_err(|error| error.note_open_pane(&pane))?;
        }

        self.start_and_prompt(&pane, &kind, &agent_args, worktree, sink)
            .map_err(|error| error.note_open_pane(&pane))
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --all-targets`
Expected: PASS, every test in the crate. Nothing pre-existing changes behaviour — the three
non-worktree placements return from `subdirectory` before any call is made.

- [ ] **Step 5: Commit**

```bash
git add src/cmd/spawn.rs
git commit -m "feat(spawn): open a worktree spawn at the caller's own subdirectory"
```

---

## Task 8: `--cwd` means one thing

The underlying defect the spec names: one flag with two meanings, distinguished by another flag's
value. The behaviour is now unified; this is the documentation catching up, and it is the part a
caller actually reads.

**Files:**
- Modify: `src/cmd/spawn.rs` (the `--cwd` help text)
- Modify: `src/herdr/surface.rs` (`create_worktree` and `worktree_args` doc comments)

- [ ] **Step 1: Write the failing test**

`--help` text is not pinned by any existing test, and pinning a help string is the kind of test that
gets weakened later. What is worth pinning is that the *stale* claim is gone. Add to
`src/cmd/spawn.rs`'s `mod tests`:

```rust
/// `--cwd` is where the agent works, under every placement.
///
/// The old help said it meant the source checkout "for a worktree" — one flag with two meanings,
/// distinguished by another flag's value, which was the defect under the subdirectory bug rather
/// than a wording slip. Asserted as an absence because the replacement wording is prose that should
/// be free to improve.
#[test]
fn the_cwd_help_no_longer_claims_a_second_meaning_for_one_placement() {
    let mut command = <Harness as clap::CommandFactory>::command();
    let rendered = command.render_help().to_string();
    // Collapsed to single spaces first: clap wraps help text to the terminal width, so a phrase
    // asserted against the raw rendering can be split across two lines and pass vacuously.
    let flattened = rendered.split_whitespace().collect::<Vec<&str>>().join(" ");

    assert!(flattened.contains("--cwd"), "the flag is still there");
    assert!(
        !flattened.contains("the checkout it is cut from"),
        "--cwd means where the agent works, for every placement: {flattened}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --all-targets spawn`
Expected: FAIL on the second assertion — the current help still carries the old clause.

- [ ] **Step 3: Update the help and the two doc comments**

In `src/cmd/spawn.rs`, replace the `cwd` field's doc comment:

```rust
    /// The directory this agent works on; defaults to the current one. A worktree spawn cuts from
    /// the repository holding it and reopens at the same relative path inside the new checkout.
    #[arg(long, value_name = "PATH")]
    cwd: Option<PathBuf>,
```

In `src/herdr/surface.rs`, replace `create_worktree`'s second paragraph:

```rust
/// `source` is a directory inside the repository the worktree is cut *from*, not where it lands —
/// herdr resolves the repository containing it, and chooses the checkout path itself under the
/// `worktree_directory` in its own config. The root pane it returns always opens at the checkout
/// root; reopening it deeper is [`open_at`]'s job and the caller's decision. `branch` and `base` are
/// herdr's to default: it generates a `worktree/`-prefixed branch name for `None` and bases on
/// `HEAD`, and restating either here would be a second authority to keep in step.
```

In `src/herdr/surface.rs`, replace the second paragraph of `worktree_args`'s doc comment:

```rust
/// `--cwd` here names a directory inside the repository to cut from — herdr resolves the repository
/// containing it — rather than the new surface's working directory, which herdr derives and always
/// puts at the checkout root. A caller that wants the agent deeper reopens the pane afterwards; see
/// [`open_at`].
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --all-targets`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/cmd/spawn.rs src/herdr/surface.rs
git commit -m "docs: give --cwd one meaning across all four placements"
```

---

## Task 9: The full verification suite, then the rehearsal

- [ ] **Step 1: Run the full static suite**

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
bash .claude/skills/rust-style/scripts/check.sh src
```

Expected: all five clean. Two notes on the last one:

- `src/cmd/spawn.rs` will be flagged as a file-length **candidate** (RS-033). It already was before
  this change. A candidate is a prompt, not a finding — it does not fail the run, and the file's
  module doc already argues why the spawn transaction stays one file.
- If the gate reports anything as a **finding**, fix it before continuing rather than explaining it.

- [ ] **Step 2: Reinstall and exercise the real binary**

`herdr-agent-tools` on `PATH` resolves to `~/.cargo/bin`, never to `target/`, so a green `cargo test`
says nothing about the command the user is about to type.

```bash
cargo install --path . --force
herdr-agent-tools spawn --help
```

Expected: the `--cwd` line reads "The directory this agent works on; defaults to the current one. A
worktree spawn cuts from the repository holding it and reopens at the same relative path inside the
new checkout." — and does not mention "the checkout it is cut from".

- [ ] **Step 3: Report the static suite, then rehearse**

Report the five commands and their results **separately** from the rehearsal below. Nothing in this
plan exercises a real herdr server, and no automated test may.

The rehearsal is the one thing worth running by hand, because the ordering against a live shell — a
`cd` landing before `agent start` types its command — is exactly what no in-crate test can cover.

1. From a subdirectory of a multi-project repository, `herdr-agent-tools spawn wtest --placement worktree`.
2. `herdr agent get wtest` and confirm both `cwd` and `foreground_cwd` report the subdirectory rather
   than the checkout root.
3. Repeat from the repository root and confirm the agent lands at the checkout root with no warning —
   the empty-relative-path case skips the step entirely.
4. Repeat with `--cwd <repo>/<a gitignored directory>` and confirm the spawn succeeds, the agent
   starts at the checkout root, and one warning names that directory.
5. `herdr-agent-tools kill wtest` after each.

**Watch for two things the tests cannot see:**

- **Path equality.** `arrived` compares `foreground_cwd` to the exact string it sent. If the shell or
  the OS reports a resolved path where herdr's checkout path is a symlink — `/tmp` against
  `/private/tmp` on macOS is the obvious one — the match never happens, the whole settle budget is
  spent, and the spawn degrades with the `NeverArrived` warning while the agent is *actually* in the
  right place. If that happens, the comparison needs to canonicalise both sides and the spec needs
  revisiting.
- **The trust dialog.** A fresh checkout is a path the agent's harness has never seen, so it may open
  on a trust prompt rather than a composer. The spec tables this deliberately and nothing here depends
  on it — but a first prompt swallowed by that dialog is what it looks like, not a bug in this change.

- [ ] **Step 4: Commit nothing**

This task changes no files. If a static check made you edit one, commit that edit on its own with a
message naming the check that demanded it.

---

## What the spec left to this plan

Recorded here so the choices are visible rather than buried in a diff. None of them re-opens a
settled decision.

1. **The style guide forbade what the spec requires.** `docs/STYLE-GUIDE.md` said shell-string
   interpolation was forbidden "anywhere", with the words "there is no exception". The spec's
   quoting rule is that exception. Task 1 amends the guide before the code arrives; the spec's
   "Where it lands" table does not list the file.
2. **The two warnings' exact wording.** The spec constrains them — name the directory, carry nothing
   else — but does not write them. Task 6 writes both and pins them by exact string, with the
   checkout path deliberately absent because the result line already reports it.
3. **A caller outside any repository, under `--placement worktree`.** The new pre-read means
   `not_git_worktree` now arrives from `worktree list` rather than from `worktree create`. Planned as
   a plain propagated `SpawnError::Herdr`, which is strictly better: it is a precondition failure, and
   nothing has been created when it fires.
4. **Which field of `worktree list`'s `source` to subtract.** The spec names `repo_root`. The response
   also carries `source_checkout_path`, and the two differ when the caller is already inside a linked
   worktree — a source herdr refuses to cut from anyway. `relative_to` answering `None` for a `cwd`
   outside the root is what keeps that case from computing nonsense in the moment before the refusal.
5. **How `foreground_cwd` is compared.** The spec names it as the success signal but not the
   comparison. Exact string equality is planned, which is the simple reading and matches the fact that
   the target string is one this crate composed. The symlink hazard is real and is the first thing the
   rehearsal watches for.
6. **The settle budget is spent twice.** `--settle-timeout` now funds two independent schedules — the
   placement retry and `agent start`'s — so a pathological launch can wait twice as long as before.
   Read from "the same budget and the same schedule", which is about the value and the shape rather
   than about sharing one countdown.
7. **`prime`'s brief is deliberately unchanged.** Its `--cwd` line already says "start it somewhere
   other than here", which was only true for three placements before and is true for all four now. The
   brief's line budget is a hard 120 with 111 lines used, and the change makes the tool behave the way
   the brief already described.
