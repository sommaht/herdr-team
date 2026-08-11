//! What a command does to an agent in a pane.

use serde::{Deserialize, Serialize};

use crate::core::{AgentName, NonEmptyText, PaneId};
use crate::herdr::{HerdrError, run, run_text};

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// herdr's status for an agent that is mid-turn, spelled once for every reader.
pub const WORKING: &str = "working";

// =====================================================================================================================
// Operations
// =====================================================================================================================

/// Reads an agent's record — one call giving both the harness kind and the current status.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
pub fn get(target: &str) -> Result<AgentRecord, HerdrError> {
    let info: AgentInfo = run(&get_args(target))?;
    Ok(info.agent)
}

/// Reads the plain-text detection snapshot the composer guard inspects.
///
/// Goes through [`run_text`]: `agent read` prints the snapshot itself, not a JSON envelope.
///
/// # Errors
///
/// Returns whatever [`run_text`] returned.
pub fn read(target: &str, source: &str, format: &str, lines: u32) -> Result<String, HerdrError> {
    run_text(&read_args(target, source, format, lines))
}

/// Starts an agent in an existing pane.
///
/// # Errors
///
/// Returns whatever [`run`] returned. `agent_pane_busy` is the retryable one: the pane exists but
/// its shell has not reached its prompt yet.
pub fn start(name: &AgentName, kind: &str, pane: &PaneId, args: &[String]) -> Result<AgentRecord, HerdrError> {
    let started: AgentStarted = run(&start_args(name, kind, pane, args))?;
    Ok(started.agent)
}

/// Submits a prompt, optionally waiting for the states that prove it was delivered.
///
/// # Errors
///
/// Returns whatever [`run`] returned. `agent_prompt_stalled` means herdr observed no state change
/// within its own 5000ms window, and `timeout` means the requested states never arrived.
pub fn prompt(target: &str, text: &NonEmptyText, wait: Option<&Wait>) -> Result<AgentRecord, HerdrError> {
    let prompted: AgentPrompted = run(&prompt_args(target, text, wait))?;
    Ok(prompted.agent)
}

/// Waits for a target to hold one of several states, with no submission attached.
///
/// Unlike the wait [`prompt`] carries, this one is unanchored: it reads the current status first
/// and returns immediately on a match, so it proves a transition only for a caller who has
/// already established one.
///
/// # Errors
///
/// Returns whatever [`run`] returned. `timeout` means none of the states arrived.
pub fn wait(target: &str, wait: &Wait) -> Result<AgentRecord, HerdrError> {
    let info: AgentInfo = run(&wait_args(target, wait))?;
    Ok(info.agent)
}

/// The delivery wait attached to a submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wait {
    /// The states that count as delivered. Each becomes one `--until`.
    pub until: Vec<String>,
    /// Milliseconds before herdr gives up.
    pub timeout: u64,
}

// =====================================================================================================================
// Responses
// =====================================================================================================================

/// An agent as herdr reports it, with only the fields this crate branches on named; everything
/// else rides through as a flattened map and is nested back into the result verbatim.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentRecord {
    /// The kind herdr detected, absent for a pane that hosts no agent. Selects the composer marker.
    agent: Option<String>,
    /// The current status, which decides whether delivery is verifiable.
    ///
    /// A `String`, not an enum: herdr owns and grows this vocabulary.
    agent_status: String,
    /// The pane the agent runs in.
    pane_id: PaneId,
    /// The name herdr recorded, absent for a pane herdr did not name.
    ///
    /// Skipped when absent: herdr omitted the field, so it stays omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Every other field herdr reported, untouched.
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

impl AgentRecord {
    /// The kind herdr detected, or `None` for a pane hosting no agent.
    pub fn kind(&self) -> Option<&str> {
        self.agent.as_deref()
    }

    /// The current status, in herdr's own spelling.
    pub fn status(&self) -> &str {
        &self.agent_status
    }

    /// The pane the agent runs in.
    pub fn pane(&self) -> &PaneId {
        &self.pane_id
    }

    /// The agent's name as herdr recorded it, or a placeholder for a pane herdr did not name.
    pub fn name_or_unknown(&self) -> &str {
        self.name.as_deref().unwrap_or("(unnamed)")
    }

    /// The agent's name as herdr recorded it, absent for a pane herdr did not name.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

/// `agent get`'s result, and `agent wait`'s — herdr answers both with the same envelope.
#[derive(Debug, Deserialize)]
struct AgentInfo {
    agent: AgentRecord,
}

/// `agent start`'s result. It also reports the `argv` it ran, which is not read here.
#[derive(Debug, Deserialize)]
struct AgentStarted {
    agent: AgentRecord,
}

/// `agent prompt`'s result.
#[derive(Debug, Deserialize)]
struct AgentPrompted {
    agent: AgentRecord,
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// `herdr agent get <TARGET>`.
fn get_args(target: &str) -> Vec<String> {
    ["agent", "get", target].map(str::to_owned).to_vec()
}

/// `herdr agent read <TARGET> --source <SOURCE> --format <FORMAT> --lines <N>`.
fn read_args(target: &str, source: &str, format: &str, lines: u32) -> Vec<String> {
    [
        "agent", "read", target, "--source", source, "--format", format, "--lines",
    ]
    .map(str::to_owned)
    .into_iter()
    .chain([lines.to_string()])
    .collect()
}

/// `herdr agent start <NAME> --kind <KIND> --pane <ID> [-- <ARGS…>]`.
///
/// The separator is omitted when there are no agent arguments.
fn start_args(name: &str, kind: &str, pane: &str, args: &[String]) -> Vec<String> {
    let mut vector = ["agent", "start", name, "--kind", kind, "--pane", pane]
        .map(str::to_owned)
        .to_vec();
    if !args.is_empty() {
        vector.push("--".to_owned());
        vector.extend(args.iter().cloned());
    }
    vector
}

/// `herdr agent prompt <TARGET> <TEXT> [--wait --until <STATE>… --timeout <MS>]`.
///
/// The text is the second positional, read before options — a prompt beginning with `--` is
/// delivered rather than misread.
fn prompt_args(target: &str, text: &str, wait: Option<&Wait>) -> Vec<String> {
    let mut vector = ["agent", "prompt", target, text].map(str::to_owned).to_vec();
    if let Some(wait) = wait {
        vector.push("--wait".to_owned());
        for state in &wait.until {
            vector.push("--until".to_owned());
            vector.push(state.clone());
        }
        vector.push("--timeout".to_owned());
        vector.push(wait.timeout.to_string());
    }
    vector
}

/// `herdr agent wait <TARGET> --until <STATE>… --timeout <MS>`.
///
/// No `--wait` flag: this subcommand *is* the wait, where `agent prompt` needs one to opt into it.
fn wait_args(target: &str, wait: &Wait) -> Vec<String> {
    let mut vector = ["agent", "wait", target].map(str::to_owned).to_vec();
    for state in &wait.until {
        vector.push("--until".to_owned());
        vector.push(state.clone());
    }
    vector.push("--timeout".to_owned());
    vector.push(wait.timeout.to_string());
    vector
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const STARTED: &str = r#"{"type":"agent_started","argv":["claude"],"agent":{"agent":"claude","agent_status":"working","pane_id":"w4:p17","name":"reviewer","tab_id":"w4:t3","workspace_id":"w4","terminal_id":"term_1","cwd":"/work","focused":false,"revision":7}}"#;

    #[test]
    fn a_start_names_the_kind_and_the_pane_and_puts_agent_args_after_the_separator() {
        assert_eq!(
            start_args(
                "reviewer",
                "claude",
                "w4:p17",
                &["--model".to_owned(), "opus".to_owned()]
            ),
            [
                "agent", "start", "reviewer", "--kind", "claude", "--pane", "w4:p17", "--", "--model", "opus"
            ]
        );
    }

    #[test]
    fn a_start_with_no_agent_args_omits_the_separator_entirely() {
        assert_eq!(
            start_args("reviewer", "claude", "w4:p17", &[]),
            ["agent", "start", "reviewer", "--kind", "claude", "--pane", "w4:p17"]
        );
    }

    #[test]
    fn a_prompt_waits_for_the_states_that_prove_delivery() {
        // herdr's bare `--wait` waits for the whole turn; `--until working` returns once the status moves.
        let wait = Wait {
            until: vec![WORKING.to_owned()],
            timeout: 15_000,
        };
        assert_eq!(
            prompt_args("reviewer", "ship it", Some(&wait)),
            [
                "agent",
                "prompt",
                "reviewer",
                "ship it",
                "--wait",
                "--until",
                "working",
                "--timeout",
                "15000"
            ]
        );
    }

    #[test]
    fn a_prompt_repeats_until_once_per_state() {
        let wait = Wait {
            until: vec!["idle".to_owned(), "blocked".to_owned()],
            timeout: 60_000,
        };
        assert_eq!(
            prompt_args("w4:p17", "go", Some(&wait)),
            [
                "agent",
                "prompt",
                "w4:p17",
                "go",
                "--wait",
                "--until",
                "idle",
                "--until",
                "blocked",
                "--timeout",
                "60000"
            ]
        );
    }

    #[test]
    fn no_verify_submits_without_waiting_for_anything() {
        assert_eq!(
            prompt_args("reviewer", "go", None),
            ["agent", "prompt", "reviewer", "go"]
        );
    }

    #[test]
    fn a_standalone_wait_repeats_until_once_per_state_and_carries_no_wait_flag() {
        let wait = Wait {
            until: vec!["idle".to_owned(), "done".to_owned()],
            timeout: 120_000,
        };

        assert_eq!(
            wait_args("reviewer", &wait),
            [
                "agent",
                "wait",
                "reviewer",
                "--until",
                "idle",
                "--until",
                "done",
                "--timeout",
                "120000"
            ]
        );
    }

    #[test]
    fn a_standalone_wait_for_one_state_names_it_once() {
        let wait = Wait {
            until: vec![WORKING.to_owned()],
            timeout: 15_000,
        };

        assert_eq!(
            wait_args("w4:p17", &wait),
            ["agent", "wait", "w4:p17", "--until", "working", "--timeout", "15000"]
        );
    }

    #[test]
    fn a_standalone_wait_answers_the_same_envelope_a_get_does() {
        let info: AgentInfo =
            serde_json::from_str(r#"{"agent":{"agent":"codex","agent_status":"done","pane_id":"w4:p17"}}"#).unwrap();

        assert_eq!(info.agent.status(), "done");
        assert_eq!(info.agent.kind(), Some("codex"));
    }

    #[test]
    fn a_read_asks_for_the_detection_snapshot_as_plain_text() {
        // `--source detection` is absent from herdr's usage line but accepted.
        assert_eq!(
            read_args("reviewer", "detection", "text", 40),
            [
                "agent",
                "read",
                "reviewer",
                "--source",
                "detection",
                "--format",
                "text",
                "--lines",
                "40"
            ]
        );
    }

    #[test]
    fn a_get_is_one_call_for_both_the_kind_and_the_status() {
        assert_eq!(get_args("reviewer"), ["agent", "get", "reviewer"]);
    }

    #[test]
    fn a_record_names_only_the_fields_the_flow_branches_on_and_carries_the_rest_untouched() {
        let started: AgentStarted = serde_json::from_str(STARTED).unwrap();
        let record = started.agent;

        assert_eq!(record.kind(), Some("claude"));
        assert_eq!(record.status(), WORKING);
        assert_eq!(record.pane(), &PaneId::from("w4:p17"));
        assert_eq!(record.name_or_unknown(), "reviewer");

        let wire: serde_json::Value = serde_json::to_value(&record).unwrap();
        assert_eq!(wire["terminal_id"], "term_1");
        assert_eq!(wire["workspace_id"], "w4");
        assert_eq!(wire["cwd"], "/work");
        assert_eq!(wire["revision"], 7);
    }

    #[test]
    fn a_pane_herdr_did_not_name_reads_as_unnamed_and_stays_absent_on_the_wire() {
        let record: AgentRecord =
            serde_json::from_str(r#"{"agent":"claude","agent_status":"idle","pane_id":"w4:p2"}"#).unwrap();

        assert_eq!(record.name_or_unknown(), "(unnamed)");
        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            r#"{"agent":"claude","agent_status":"idle","pane_id":"w4:p2"}"#,
            "herdr omitted the field, so it stays omitted"
        );
    }

    #[test]
    fn a_record_reports_its_raw_name_so_a_caller_can_choose_its_own_fallback() {
        let named: AgentRecord =
            serde_json::from_str(r#"{"agent":"claude","agent_status":"idle","pane_id":"w4:p2","name":"reviewer"}"#)
                .unwrap();
        assert_eq!(named.name(), Some("reviewer"));

        let unnamed: AgentRecord =
            serde_json::from_str(r#"{"agent":"claude","agent_status":"idle","pane_id":"w4:p2"}"#).unwrap();
        assert_eq!(unnamed.name(), None);
        assert_eq!(unnamed.name_or_unknown(), "(unnamed)");
    }

    #[test]
    fn a_pane_with_no_agent_reports_no_kind_rather_than_failing_to_parse() {
        let record: AgentRecord =
            serde_json::from_str(r#"{"agent":null,"agent_status":"unknown","pane_id":"w4:p2"}"#).unwrap();

        assert_eq!(record.kind(), None);
        assert_eq!(record.status(), "unknown");
    }

    #[test]
    fn a_status_herdr_adds_later_parses_and_round_trips() {
        let record: AgentRecord =
            serde_json::from_str(r#"{"agent":"claude","agent_status":"compacting","pane_id":"w4:p2"}"#).unwrap();

        assert_eq!(record.status(), "compacting");
        assert_eq!(serde_json::to_value(&record).unwrap()["agent_status"], "compacting");
    }
}
