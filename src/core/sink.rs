//! The output seam: every command result and every diagnostic passes through here.
//!
//! Commands do not print. They return a value and push diagnostics, and this renders both — as
//! human text or as tagged NDJSON — so the `--json` contract lives in one place rather than at
//! every write site.

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
/// A concrete struct rather than a trait: [`Sink::out`] is generic over `Display + Serialize`, and
/// a generic method is not object-safe, so a `Box<dyn Sink>` would not compile. Owning the writers
/// is also what makes output assertable in tests.
///
/// Writes are best-effort. A failed write to stdout — a closed pipe, say — is swallowed rather than
/// panicking, which is what `println!` would do.
pub struct Sink {
    mode: OutputMode,
    out: RefCell<Box<dyn Write>>,
    err: RefCell<Box<dyn Write>>,
}

impl Sink {
    /// A sink over the process's own streams.
    ///
    /// In [`OutputMode::Json`] both handles are stdout: a machine consumer reading two streams
    /// cannot rely on their relative ordering once either is redirected, and the `type` tag already
    /// carries what the stream choice would have.
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
    /// In [`OutputMode::Json`] the value's own fields are flattened into the tagged object, so a
    /// consumer reads `line["placement"]` rather than `line["data"]["placement"]`. That requires
    /// the value to serialize as a JSON object, which every command result does.
    pub fn out<T: Display + Serialize + ?Sized>(&self, value: &T) {
        self.emit(&self.out, "result", None, value);
    }

    /// Emits a diagnostic — a warning on a path that still succeeds.
    ///
    /// Takes a string rather than a value, because a warning *is* a sentence: there is no
    /// structure under it worth flattening, and the three warnings this crate emits say the
    /// delivery was unverified or the composer could not be read. Separate from a `Result`'s `Err`
    /// so a warning does not have to be spelled as a failure — it does not decide the exit status.
    ///
    /// In [`OutputMode::Json`] this goes to the *result* stream, so the whole run is one ordered
    /// NDJSON document, and the `type` tag is what separates a warning from a result.
    pub fn warn(&self, message: &str) {
        self.emit(self.diagnostic_stream(), "warning", None, &Warning { message });
    }

    /// Emits a command failure, carrying the exit status it maps to.
    ///
    /// The status rides in the wire form so a consumer that already has the line does not also
    /// have to read `$?`. It is passed in rather than read off the value because the value is a
    /// rendering of the failure, and the exit status belongs to the process.
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
                // Only reachable for a value that does not serialize as a JSON object, which the
                // flatten in `Tagged` requires. Reported as a line rather than swallowed, because a
                // consumer reading NDJSON would otherwise see a silently missing record.
                Err(error) => write_line(stream, &format!(r#"{{"type":"error","status":1,"message":"{error}"}}"#)),
            },
        }
    }
}

// =====================================================================================================================
// Wire Forms
// =====================================================================================================================

/// One NDJSON line: the tag, the exit status where there is one, then the value's own fields.
///
/// `#[serde(flatten)]` rather than a hand-built `serde_json::Map`, so the wire shape stays
/// described by derives (RS-021) and field order stays declaration order.
#[derive(Serialize)]
struct Tagged<'a, T: Serialize + ?Sized> {
    #[serde(rename = "type")]
    tag: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u8>,
    #[serde(flatten)]
    value: &'a T,
}

/// A warning's wire form — the one output the sink shapes itself, since a warning is a sentence.
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

/// Writes one rendered line to a stream and flushes it.
///
/// The single place an output failure is swallowed, and the only place it can be justified: the
/// sink *is* the reporting channel, so there is nowhere to report a failed write to, and the
/// command whose result this is has already succeeded. `println!` would panic on the same closed
/// pipe.
fn write_line<T: Display + ?Sized>(stream: &RefCell<Box<dyn Write>>, line: &T) {
    let mut stream = stream.borrow_mut();
    let _ = writeln!(stream, "{line}");
    let _ = stream.flush();
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

    /// A writer the test can read back, since `Sink` owns its writers.
    #[derive(Clone, Default)]
    struct SharedBuf(Rc<RefCell<Vec<u8>>>);

    impl SharedBuf {
        fn contents(&self) -> String {
            String::from_utf8(self.0.borrow().clone()).unwrap()
        }
    }

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

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
        // The tag and the status lead, then the value's own fields in declaration order — the shape
        // the design pins. Nesting under a `data` key would make a consumer unwrap every line.
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
        // One ordered NDJSON document: a consumer cannot rely on two streams' relative ordering
        // once either is redirected, and the tag carries what the stream choice would have said.
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
