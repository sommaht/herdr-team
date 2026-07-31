1) Always follow the style guide @docs/STYLE-GUIDE.md.

2) **herdr is the authority for its own CLI syntax.** Print a command group to read it —
   `herdr agent`, `herdr pane`, `herdr tab`, `herdr workspace`, `herdr worktree` — rather than
   guessing a flag. Leaf subcommands fall back to top-level help, so read the group. Never run
   bare `herdr`: it launches or attaches the TUI.

3) **Never put a prompt, an agent's arguments, or captured terminal content in an error
   message, a diagnostic, or a log.** The argument to `herdr agent prompt` is the prompt text,
   and the composer guard's input is a snapshot of someone's half-written message. A refusal
   says the composer holds unsent text; it never says what that text is.

4) Verify against a live herdr session only when a change cannot be covered in-crate, and
   report that rehearsal separately from the static checks. Automated tests never invoke herdr.

5) **`cargo install --path . --force` is the last step of the verification suite**, whenever a
   change touched the binary. `herdr-team` on `PATH` resolves to `~/.cargo/bin`, never to
   `target/`, so a green `cargo test` says nothing about the command the user is about to type.
   Exercise the changed surface through the installed binary afterwards — a stale `--help` string
   passes every test in the crate.
