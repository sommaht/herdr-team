1) Always follow the style guide @docs/STYLE-GUIDE.md.

2) **herdr is the authority for its own CLI syntax.** Print a command group to read it —
   `herdr agent`, `herdr pane`, `herdr tab`, `herdr workspace` — rather than guessing a flag.
   Leaf subcommands fall back to top-level help, so read the group. Never run bare `herdr`:
   it launches or attaches the TUI.

3) **Never put a prompt, a preset's arguments, or captured terminal content in an error
   message, a diagnostic, or a log.** The argument to `herdr agent prompt` is the prompt text,
   and the composer guard's input is a snapshot of someone's half-written message. A refusal
   says the composer holds unsent text; it never says what that text is.

4) Verify against a live herdr session only when a change cannot be covered in-crate, and
   report that rehearsal separately from the static checks. Automated tests never invoke herdr.
