//! The output seam: every command result and every diagnostic renders here, as human text or as
//! tagged NDJSON.

use std::cell::RefCell;
use std::fmt::Display;
use std::io::{self, Write};

use serde::Serialize;

// =====================================================================================================================
// OutputMode
// =====================================================================================================================

/// How a [`Sink`] renders what it is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputMode {
    /// One line of `Display` text per value; results to stdout, diagnostics to stderr.
    Human,
    /// Tagged NDJSON, everything on stdout — see [`Sink`].
    Json,
}

// =====================================================================================================================
// Sink
// =====================================================================================================================

/// Where a command's output goes, and in which form.
///
/// A concrete struct because [`Sink::out`] is generic, and a generic method is not object-safe.
pub struct Sink {
    mode: OutputMode,
    out: RefCell<Box<dyn Write>>,
    err: RefCell<Box<dyn Write>>,
}

impl Sink {
    /// A sink over the process's own streams; in [`OutputMode::Json`] both handles are stdout.
    pub fn new(mode: OutputMode) -> Self {
        let err: Box<dyn Write> = match mode {
            OutputMode::Human => Box::new(io::stderr()),
            OutputMode::Json => Box::new(io::stdout()),
        };
        Self::with_writers(mode, Box::new(io::stdout()), err)
    }

    /// A sink over caller-supplied writers.
    pub fn with_writers(mode: OutputMode, out: Box<dyn Write>, err: Box<dyn Write>) -> Self {
        Self {
            mode,
            out: RefCell::new(out),
            err: RefCell::new(err),
        }
    }

    /// Emits a command's result.
    ///
    /// In [`OutputMode::Json`] the value's own fields are flattened into the tagged object, so the
    /// value must serialize as a JSON object.
    pub fn out<T: Display + Serialize + ?Sized>(&self, value: &T) {
        self.emit(&self.out, "result", None, value);
    }

    /// Emits a result that is the same text in either mode — see
    /// [`Cmd::TEXT_IN_BOTH_MODES`](crate::cmd::Cmd::TEXT_IN_BOTH_MODES).
    pub fn out_text<T: Display + ?Sized>(&self, value: &T) {
        write_line(&self.out, value);
    }

    /// Emits a diagnostic — a warning on a path that still succeeds.
    ///
    /// In [`OutputMode::Json`] it joins the *result* stream, tagged, so the run stays one ordered
    /// NDJSON document.
    pub fn warn(&self, message: &str) {
        self.emit(self.diagnostic_stream(), "warning", None, &Warning { message });
    }

    /// Emits a command failure, carrying the exit status it maps to.
    pub fn error<T: Display + Serialize + ?Sized>(&self, value: &T, status: u8) {
        self.emit(self.diagnostic_stream(), "error", Some(status), value);
    }

    /// The stream diagnostics go to: stderr for a human, the single stdout stream for a machine.
    fn diagnostic_stream(&self) -> &RefCell<Box<dyn Write>> {
        match self.mode {
            OutputMode::Human => &self.err,
            OutputMode::Json => &self.out,
        }
    }

    /// Renders one value onto one stream, in this sink's mode.
    fn emit<T: Display + Serialize + ?Sized>(
        &self,
        stream: &RefCell<Box<dyn Write>>,
        tag: &'static str,
        status: Option<u8>,
        value: &T,
    ) {
        match self.mode {
            OutputMode::Human => write_line(stream, value),
            OutputMode::Json => match serde_json::to_string(&Tagged { tag, status, value }) {
                Ok(line) => write_line(stream, &line),
                // Only reachable for a value that is not a JSON object; reported rather than
                // silently dropped from the NDJSON stream.
                Err(error) => write_line(stream, &format!(r#"{{"type":"error","status":1,"message":"{error}"}}"#)),
            },
        }
    }
}

// =====================================================================================================================
// Wire Forms
// =====================================================================================================================

/// One NDJSON line: the tag, the exit status where there is one, then the value's own fields.
#[derive(Serialize)]
struct Tagged<'a, T: Serialize + ?Sized> {
    #[serde(rename = "type")]
    tag: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u8>,
    #[serde(flatten)]
    value: &'a T,
}

/// A warning's wire form — the one output the sink shapes itself.
#[derive(Serialize)]
struct Warning<'a> {
    message: &'a str,
}

impl Display for Warning<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// Writes one rendered line to a stream and flushes it — the one place a failed write is swallowed.
fn write_line<T: Display + ?Sized>(stream: &RefCell<Box<dyn Write>>, line: &T) {
    let mut stream = stream.borrow_mut();
    let _ = writeln!(stream, "{line}");
    let _ = stream.flush();
}

// =====================================================================================================================
// Test support
// =====================================================================================================================

/// A writer a test can read back, since [`Sink`] owns its writers.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct SharedBuf(std::rc::Rc<RefCell<Vec<u8>>>);

#[cfg(test)]
impl SharedBuf {
    /// Everything written so far.
    pub(crate) fn contents(&self) -> String {
        String::from_utf8(self.0.borrow().clone()).unwrap()
    }
}

#[cfg(test)]
impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Spawned {
        placement: &'static str,
        delivered: bool,
    }

    impl Display for Spawned {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "reviewer (claude) → w4:p17")
        }
    }

    fn spawned() -> Spawned {
        Spawned { placement: "tab", delivered: true }
    }

    /// A sink writing into two buffers the test keeps handles on.
    fn sink(mode: OutputMode) -> (Sink, SharedBuf, SharedBuf) {
        let out = SharedBuf::default();
        let err = SharedBuf::default();
        let sink = Sink::with_writers(mode, Box::new(out.clone()), Box::new(err.clone()));
        (sink, out, err)
    }

    #[test]
    fn human_mode_writes_the_display_form_to_the_matching_stream() {
        let (sink, out, err) = sink(OutputMode::Human);

        sink.out(&spawned());
        assert_eq!(out.contents(), "reviewer (claude) → w4:p17\n");
        assert_eq!(err.contents(), "");

        sink.warn("delivery could not be verified");
        assert_eq!(err.contents(), "delivery could not be verified\n");
    }

    #[test]
    fn json_mode_flattens_the_value_into_the_tagged_object() {
        let (sink, out, err) = sink(OutputMode::Json);

        sink.out(&spawned());

        assert_eq!(
            out.contents(),
            "{\"type\":\"result\",\"placement\":\"tab\",\"delivered\":true}\n"
        );
        assert_eq!(err.contents(), "", "json mode puts everything on stdout");
    }

    #[test]
    fn json_mode_puts_warnings_on_the_result_stream_with_their_own_tag() {
        let (sink, out, err) = sink(OutputMode::Json);

        sink.warn("composer could not be located");

        assert_eq!(
            out.contents(),
            "{\"type\":\"warning\",\"message\":\"composer could not be located\"}\n"
        );
        assert_eq!(err.contents(), "");
    }

    #[test]
    fn a_failure_renders_as_a_line_for_humans_and_a_tagged_object_carrying_its_status() {
        #[derive(Serialize)]
        struct Failure {
            message: &'static str,
            herdr: Herdr,
        }
        #[derive(Serialize)]
        struct Herdr {
            command: &'static str,
            code: &'static str,
        }
        impl Display for Failure {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.message)
            }
        }
        let failure = Failure {
            message: "agent target pane w4:p16 is not an available shell",
            herdr: Herdr {
                command: "agent start",
                code: "agent_pane_busy",
            },
        };

        let (human, _out, err) = sink(OutputMode::Human);
        human.error(&failure, 5);
        assert_eq!(err.contents(), "agent target pane w4:p16 is not an available shell\n");

        let (json, out, _err) = sink(OutputMode::Json);
        json.error(&failure, 5);
        assert_eq!(
            out.contents(),
            "{\"type\":\"error\",\"status\":5,\
             \"message\":\"agent target pane w4:p16 is not an available shell\",\
             \"herdr\":{\"command\":\"agent start\",\"code\":\"agent_pane_busy\"}}\n"
        );
    }

    #[test]
    fn json_mode_emits_one_self_contained_object_per_line() {
        let (sink, out, _err) = sink(OutputMode::Json);

        sink.warn("first");
        sink.out(&spawned());

        assert_eq!(out.contents().lines().count(), 2);
        for line in out.contents().lines() {
            serde_json::from_str::<serde_json::Value>(line).expect("each line parses alone");
        }
    }
}
