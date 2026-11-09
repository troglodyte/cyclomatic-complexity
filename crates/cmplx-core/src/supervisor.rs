//! Scores sources in a worker child process (see [`crate::worker`]), so that
//! an analyzer that overflows its stack, hangs or dies fails one source
//! instead of the whole process. A stack overflow aborts a process outright;
//! `catch_unwind` cannot stop it, so the only containment is a process
//! boundary.
//!
//! One worker serves one request at a time. When it dies, closes its output,
//! answers nonsense or takes longer than the timeout, it is killed and reaped,
//! the source in flight fails with [`ParseErrorKind::Crashed`], and the next
//! source starts a fresh worker, up to a restart limit. If the first worker
//! cannot be started, the supervisor says so once and scores in process; a
//! restart that cannot be started fails its source, and counts as a restart.
//!
//! Generic over languages: it names none, and sends each source to the
//! worker under its language's id.

use crate::language::{analyze_bounded, Lang, LangOptions};
use crate::model::{CrashReason, Func, ParseError, ParseErrorKind, Termination};
use crate::run::MAX_FILE_BYTES;
use crate::worker::{self, Line, Request, WORKER_FLAG};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// Longest a worker may take over one source before it is killed.
pub const WORKER_TIMEOUT: Duration = Duration::from_secs(10);

/// Most worker restarts in one CLI run, and in any [`RESTART_WINDOW`] of a
/// language server session.
pub const MAX_WORKER_RESTARTS: usize = 10;

/// The rolling window the language server counts restarts in.
pub const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// Most worker stderr kept per request; the rest is read and dropped so the
/// worker never blocks on a full pipe.
pub const MAX_WORKER_STDERR: usize = 64 * 1024;

/// Longest response line read. A function needs a few source bytes and its
/// JSON row takes tens of bytes, so this leaves a wide margin over any
/// response to a source of [`MAX_FILE_BYTES`].
const MAX_RESPONSE_BYTES: u64 = MAX_FILE_BYTES * 64;

/// How long a worker that closed its output gets to exit before it is killed.
const EXIT_GRACE: Duration = Duration::from_secs(1);

/// How often an exiting worker is polled within [`EXIT_GRACE`].
const EXIT_POLL: Duration = Duration::from_millis(5);

/// Polls that fit in [`EXIT_GRACE`].
const EXIT_POLLS: u32 = 200;

/// How long to wait, once a worker is reaped, for the rest of its stderr.
/// Only a grandchild still holding the pipe makes this wait at all.
const STDERR_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// Read size for worker stderr.
const STDERR_CHUNK_BYTES: usize = 8 * 1024;

/// Longest stderr line quoted in a crash message, in chars.
const MAX_QUOTED_STDERR_CHARS: usize = 200;

/// What the Rust runtime prints before aborting on a stack overflow.
const STACK_OVERFLOW_TEXT: &str = "has overflowed its stack";

/// How many worker restarts are allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartLimit {
    /// At most `max` in the supervisor's life: the CLI.
    PerRun { max: usize },
    /// At most `max` in any rolling `window`: the language server.
    PerWindow { max: usize, window: Duration },
}

/// How a supervisor runs its worker. Injectable so tests can use fake workers.
#[derive(Clone, Debug)]
pub struct SupervisorConfig {
    /// Program and arguments; `None` is this executable with `--worker`.
    pub command: Option<Vec<OsString>>,
    pub timeout: Duration,
    pub restarts: RestartLimit,
}

impl SupervisorConfig {
    /// For one CLI run.
    pub fn cli() -> SupervisorConfig {
        SupervisorConfig {
            command: None,
            timeout: WORKER_TIMEOUT,
            restarts: RestartLimit::PerRun {
                max: MAX_WORKER_RESTARTS,
            },
        }
    }

    /// For a language server session.
    pub fn lsp() -> SupervisorConfig {
        SupervisorConfig {
            restarts: RestartLimit::PerWindow {
                max: MAX_WORKER_RESTARTS,
                window: RESTART_WINDOW,
            },
            ..SupervisorConfig::cli()
        }
    }

    fn program(&self) -> io::Result<Vec<OsString>> {
        match &self.command {
            Some(command) if !command.is_empty() => Ok(command.clone()),
            Some(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty worker command",
            )),
            None => Ok(vec![
                std::env::current_exe()?.into_os_string(),
                WORKER_FLAG.into(),
            ]),
        }
    }
}

enum Mode {
    Supervised(SupervisorConfig),
    /// No worker could be started, or none was wanted: score in process.
    InProcess,
}

/// Scores sources through a worker process. See the module docs.
pub struct Supervisor {
    mode: Mode,
    worker: Option<Worker>,
    /// Workers started so far; every one after the first is a restart.
    spawned: usize,
    /// When each restart happened, oldest first; never more than the limit.
    restarts: VecDeque<Instant>,
    next_id: u64,
    warnings: Vec<String>,
}

/// Why no worker is available for a source.
enum Unavailable {
    GaveUp,
    Spawn(io::Error),
}

impl Supervisor {
    pub fn new(config: SupervisorConfig) -> Supervisor {
        Supervisor {
            mode: Mode::Supervised(config),
            worker: None,
            spawned: 0,
            restarts: VecDeque::new(),
            next_id: 0,
            warnings: Vec::new(),
        }
    }

    /// Scores in process, unprotected: for tests of the code around it.
    pub fn in_process() -> Supervisor {
        Supervisor {
            mode: Mode::InProcess,
            ..Supervisor::new(SupervisorConfig::cli())
        }
    }

    /// Workers started so far.
    pub fn spawned(&self) -> usize {
        self.spawned
    }

    /// Warnings to show once each, such as falling back to in-process scoring.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// Score `src` as [`analyze_bounded`] would, in the worker.
    pub fn analyze(
        &mut self,
        lang: Lang,
        src: &str,
        path: &Path,
        opts: &LangOptions,
    ) -> Result<Vec<Func>, ParseError> {
        let config = match &self.mode {
            Mode::InProcess => return analyze_bounded(lang, src, path, opts),
            Mode::Supervised(config) => config.clone(),
        };
        let mut worker = match self.take_worker(&config) {
            Ok(worker) => worker,
            Err(Unavailable::GaveUp) => return Err(crashed(lang, CrashReason::GaveUp, &config)),
            // No worker has ever started, so none will: say so once and
            // score in process from now on.
            Err(Unavailable::Spawn(e)) if self.spawned == 0 => {
                self.warnings
                    .push(format!("running without crash protection: {e}"));
                self.mode = Mode::InProcess;
                return analyze_bounded(lang, src, path, opts);
            }
            // A restart failed. Scoring in process now would drop protection
            // exactly when an analyzer is crashing, so fail this source and
            // let the restart limit decide about the next.
            Err(Unavailable::Spawn(e)) => {
                let reason = CrashReason::SpawnFailed {
                    error: e.to_string(),
                };
                return Err(crashed(lang, reason, &config));
            }
        };
        self.next_id += 1;
        let request = worker::encode_request(&Request {
            id: self.next_id,
            lang: lang.id(),
            path,
            text: src,
            opts,
        });
        match worker.exchange(request, self.next_id, config.timeout) {
            Ok(result) => {
                self.worker = Some(worker);
                result
            }
            Err(failure) => Err(crashed(lang, worker.finish(failure), &config)),
        }
    }

    /// The running worker, or a new one if the restart limit allows.
    fn take_worker(&mut self, config: &SupervisorConfig) -> Result<Worker, Unavailable> {
        if let Some(worker) = self.worker.take() {
            return Ok(worker);
        }
        if self.spawned > 0 {
            self.claim_restart(config.restarts)?;
        }
        let program = config.program().map_err(Unavailable::Spawn)?;
        let worker = Worker::spawn(&program).map_err(Unavailable::Spawn)?;
        self.spawned += 1;
        Ok(worker)
    }

    /// Record a restart, unless that would break the limit.
    fn claim_restart(&mut self, limit: RestartLimit) -> Result<(), Unavailable> {
        let now = Instant::now();
        let max = match limit {
            RestartLimit::PerRun { max } => max,
            RestartLimit::PerWindow { max, window } => {
                self.restarts.retain(|at| now.duration_since(*at) < window);
                max
            }
        };
        if self.restarts.len() >= max {
            return Err(Unavailable::GaveUp);
        }
        self.restarts.push_back(now);
        Ok(())
    }
}

/// The failure for a source the worker did not score.
fn crashed(lang: Lang, reason: CrashReason, config: &SupervisorConfig) -> ParseError {
    ParseError {
        line: 0,
        message: crash_message(lang.display_name(), &reason, config),
        kind: ParseErrorKind::Crashed { reason },
    }
}

/// The text shown for a source that was not scored, after `<program>: <path>: `.
fn crash_message(lang: &str, reason: &CrashReason, config: &SupervisorConfig) -> String {
    match reason {
        CrashReason::StackOverflow => format!(
            "not scored: the {lang} analyzer crashed (stack overflow); \
             the file is probably nested too deeply to analyze"
        ),
        CrashReason::Timeout => format!(
            "not scored: the {lang} analyzer took longer than {}",
            duration_label(config.timeout)
        ),
        CrashReason::Exited {
            termination,
            stderr,
        } => {
            let how = termination_label(*termination);
            let quoted: String = stderr.chars().take(MAX_QUOTED_STDERR_CHARS).collect();
            match quoted.is_empty() {
                true => format!("not scored: the {lang} analyzer crashed ({how})"),
                false => format!("not scored: the {lang} analyzer crashed ({how}): {quoted}"),
            }
        }
        CrashReason::BadResponse { detail } => {
            format!("not scored: the {lang} analyzer sent a bad response: {detail}")
        }
        CrashReason::SpawnFailed { error } => {
            format!("not scored: the {lang} analyzer could not be restarted: {error}")
        }
        CrashReason::GaveUp => match config.restarts {
            RestartLimit::PerRun { max } => {
                format!("not scored: the analyzer kept crashing; gave up after {max} restarts")
            }
            RestartLimit::PerWindow { window, .. } => format!(
                "not scored: the analyzer kept crashing; restarting is paused for {}",
                window_label(window)
            ),
        },
    }
}

/// `10 s`, or `250 ms` for a fraction of a second.
fn duration_label(d: Duration) -> String {
    match d.subsec_millis() {
        0 => format!("{} s", d.as_secs()),
        _ => format!("{} ms", d.as_millis()),
    }
}

/// `a minute` for [`RESTART_WINDOW`], else a [`duration_label`].
fn window_label(d: Duration) -> String {
    match d == RESTART_WINDOW {
        true => "a minute".to_string(),
        false => duration_label(d),
    }
}

fn termination_label(termination: Termination) -> String {
    match termination {
        Termination::Code(code) => format!("exit code {code}"),
        Termination::Signal(signal) => format!("signal {}", signal_name(signal)),
        Termination::Unknown => "no exit status".to_string(),
    }
}

/// Names for the signals a crash usually ends in.
const SIGNAL_NAMES: &[(i32, &str)] = &[
    (4, "SIGILL"),
    (5, "SIGTRAP"),
    (SIGABRT, "SIGABRT"),
    (8, "SIGFPE"),
    (9, "SIGKILL"),
    (SIGBUS, "SIGBUS"),
    (SIGSEGV, "SIGSEGV"),
    (13, "SIGPIPE"),
    (15, "SIGTERM"),
];

/// `SIGABRT` and the like, or the number for a signal not in [`SIGNAL_NAMES`].
fn signal_name(signal: i32) -> String {
    SIGNAL_NAMES
        .iter()
        .find(|(number, _)| *number == signal)
        .map_or_else(|| signal.to_string(), |(_, name)| name.to_string())
}

#[cfg(target_os = "linux")]
const SIGBUS: i32 = 7;
#[cfg(not(target_os = "linux"))]
const SIGBUS: i32 = 10;

/// The signals a stack overflow ends in: the runtime's abort, or a guard-page
/// fault when the runtime cannot report it.
const SIGABRT: i32 = 6;
const SIGSEGV: i32 = 11;

/// How a worker that ended on its own failed, from its exit and its stderr.
fn classify(termination: Termination, stderr: &str) -> CrashReason {
    let overflow_signal = matches!(termination, Termination::Signal(SIGABRT | SIGSEGV));
    if overflow_signal && stderr.contains(STACK_OVERFLOW_TEXT) {
        return CrashReason::StackOverflow;
    }
    CrashReason::Exited {
        termination,
        stderr: first_line(stderr),
    }
}

/// The first line with anything but whitespace on it, trimmed.
fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

#[cfg(unix)]
fn termination(status: &ExitStatus) -> Termination {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => Termination::Code(code),
        (None, Some(signal)) => Termination::Signal(signal),
        (None, None) => Termination::Unknown,
    }
}

#[cfg(not(unix))]
fn termination(status: &ExitStatus) -> Termination {
    status
        .code()
        .map_or(Termination::Unknown, Termination::Code)
}

/// What the worker's reader and writer threads report.
enum Event {
    /// One response line.
    Response(Vec<u8>),
    /// A response line over [`MAX_RESPONSE_BYTES`].
    TooLong,
    /// Its stdout ended or failed, or a request could not be written: it is
    /// gone or going.
    Closed,
}

/// Why an exchange with the worker failed.
enum Failure {
    Closed,
    Timeout,
    BadResponse(String),
}

/// One running worker process and the threads that talk to it.
struct Worker {
    child: Child,
    /// To the writer thread; dropping it closes the worker's stdin.
    requests: Option<Sender<Vec<u8>>>,
    events: Receiver<Event>,
    stderr: Arc<Mutex<Vec<u8>>>,
    /// Signalled once the stderr thread has read to the end.
    stderr_done: Receiver<()>,
}

impl Worker {
    fn spawn(program: &[OsString]) -> io::Result<Worker> {
        let (exe, args) = program
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty worker command"))?;
        let mut child = Command::new(exe)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        match Worker::attach(&mut child) {
            Ok((requests, events, stderr, stderr_done)) => Ok(Worker {
                child,
                requests: Some(requests),
                events,
                stderr,
                stderr_done,
            }),
            Err(e) => {
                // The worker is useless without its threads; reap it and report
                // why it could not be used, not how it ended.
                let _status = stop(&mut child);
                Err(e)
            }
        }
    }

    /// Start the threads that write requests, read responses and collect stderr.
    #[allow(clippy::type_complexity)]
    fn attach(
        child: &mut Child,
    ) -> io::Result<(
        Sender<Vec<u8>>,
        Receiver<Event>,
        Arc<Mutex<Vec<u8>>>,
        Receiver<()>,
    )> {
        let missing = |name| io::Error::other(format!("the worker has no {name} pipe"));
        let stdin = child.stdin.take().ok_or_else(|| missing("stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| missing("stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| missing("stderr"))?;
        let (requests, requests_rx) = mpsc::channel();
        let (events_tx, events) = mpsc::channel();
        let (done_tx, stderr_done) = mpsc::channel();
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer_events = events_tx.clone();
        let collected = Arc::clone(&buffer);
        thread::Builder::new()
            .name("cmplx-worker-stdin".into())
            .spawn(move || write_requests(stdin, requests_rx, writer_events))?;
        thread::Builder::new()
            .name("cmplx-worker-stdout".into())
            .spawn(move || read_responses(stdout, events_tx))?;
        thread::Builder::new()
            .name("cmplx-worker-stderr".into())
            .spawn(move || collect_stderr(stderr, collected, done_tx))?;
        Ok((requests, events, buffer, stderr_done))
    }

    /// Send one request and wait up to `timeout` for its response.
    fn exchange(
        &mut self,
        request: String,
        id: u64,
        timeout: Duration,
    ) -> Result<Result<Vec<Func>, ParseError>, Failure> {
        lock(&self.stderr).clear();
        let requests = self.requests.as_ref().ok_or(Failure::Closed)?;
        // A failed send means the writer thread ended: the worker's stdin broke.
        requests
            .send(request.into_bytes())
            .map_err(|_| Failure::Closed)?;
        match self.events.recv_timeout(timeout) {
            Ok(Event::Response(line)) => {
                worker::decode_response(&line, id).map_err(Failure::BadResponse)
            }
            Ok(Event::TooLong) => Err(Failure::BadResponse(format!(
                "a response longer than {MAX_RESPONSE_BYTES} bytes"
            ))),
            Ok(Event::Closed) | Err(RecvTimeoutError::Disconnected) => Err(Failure::Closed),
            Err(RecvTimeoutError::Timeout) => Err(Failure::Timeout),
        }
    }

    /// Reap a worker that failed, and say how it failed.
    fn finish(mut self, failure: Failure) -> CrashReason {
        self.requests = None;
        match failure {
            Failure::Timeout => {
                let _status = stop(&mut self.child);
                CrashReason::Timeout
            }
            Failure::BadResponse(detail) => {
                let _status = stop(&mut self.child);
                CrashReason::BadResponse { detail }
            }
            Failure::Closed => {
                let termination = match exit_within_grace(&mut self.child) {
                    Ok(status) => termination(&status),
                    Err(_) => Termination::Unknown,
                };
                classify(termination, &self.drained_stderr())
            }
        }
    }

    /// The stderr of the request in flight, once the worker has gone.
    fn drained_stderr(&self) -> String {
        // Done or disconnected: the buffer is final. Timed out: a grandchild
        // holds the pipe open, so use what arrived in time.
        let _drained = self.stderr_done.recv_timeout(STDERR_DRAIN_TIMEOUT);
        String::from_utf8_lossy(&lock(&self.stderr)).into_owned()
    }
}

impl Drop for Worker {
    /// No worker outlives its supervisor, and none is left a zombie.
    fn drop(&mut self) {
        self.requests = None;
        // Nothing can be reported from a drop; a failure here means the
        // child is already reaped or cannot be signalled at all.
        let _status = stop(&mut self.child);
    }
}

/// The worker's status: as it already ended, else after killing it.
fn stop(child: &mut Child) -> io::Result<ExitStatus> {
    if let Some(status) = child.try_wait()? {
        return Ok(status);
    }
    child.kill()?;
    child.wait()
}

/// The status of a worker that closed its output, giving it [`EXIT_GRACE`]
/// to exit on its own (a crash closes the pipe a moment before the process
/// is reapable) before it is killed.
fn exit_within_grace(child: &mut Child) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + EXIT_GRACE;
    for _ in 0..EXIT_POLLS {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(EXIT_POLL);
    }
    stop(child)
}

/// Append as much of `bytes` as fits under [`MAX_WORKER_STDERR`].
fn keep_capped(kept: &mut Vec<u8>, bytes: &[u8]) {
    let room = MAX_WORKER_STDERR.saturating_sub(kept.len());
    kept.extend_from_slice(&bytes[..bytes.len().min(room)]);
}

fn lock(buffer: &Mutex<Vec<u8>>) -> std::sync::MutexGuard<'_, Vec<u8>> {
    // A panic elsewhere cannot leave a byte buffer inconsistent.
    buffer.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Write each request to the worker until the supervisor drops its sender
/// (one iteration per request) or a write fails.
fn write_requests(mut stdin: ChildStdin, requests: Receiver<Vec<u8>>, events: Sender<Event>) {
    for request in requests {
        if stdin
            .write_all(&request)
            .and_then(|()| stdin.flush())
            .is_err()
        {
            // The worker's stdin is gone; so, in effect, is the worker. A
            // failed send means the supervisor has already finished with it.
            events.send(Event::Closed).ok();
            return;
        }
    }
}

/// Pass each response line on until stdout ends (one iteration per response,
/// bounded by the worker's life) or the supervisor stops listening.
fn read_responses(stdout: ChildStdout, events: Sender<Event>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let (event, last) = match worker::read_line_capped(&mut reader, MAX_RESPONSE_BYTES) {
            Ok(Line::Text(line)) => (Event::Response(line), false),
            Ok(Line::TooLong) => (Event::TooLong, true),
            Ok(Line::Eof) | Err(_) => (Event::Closed, true),
        };
        if events.send(event).is_err() || last {
            return;
        }
    }
}

/// Keep the first [`MAX_WORKER_STDERR`] bytes of stderr since the last clear,
/// reading and dropping the rest, until stderr ends (bounded by the worker's
/// life). Then signal `done`.
fn collect_stderr(mut stderr: ChildStderr, buffer: Arc<Mutex<Vec<u8>>>, done: Sender<()>) {
    let mut chunk = [0u8; STDERR_CHUNK_BYTES];
    loop {
        match stderr.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => keep_capped(&mut lock(&buffer), &chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let mut kept = lock(&buffer);
                kept.extend_from_slice(format!("\n(stderr unreadable: {e})\n").as_bytes());
                break;
            }
        }
    }
    // A failed send means the supervisor stopped waiting; nobody needs to know.
    done.send(()).ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::{Language, OptionSpec};
    use crate::model::Kind;

    /// Scores every source as one function `f` of complexity 1, in process.
    struct Fake;
    impl Language for Fake {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn display_name(&self) -> &'static str {
            "Fake"
        }
        fn alias(&self) -> Option<&'static str> {
            None
        }
        fn extensions(&self) -> &'static [&'static str] {
            &["fk"]
        }
        fn stdin_ext(&self) -> &'static str {
            "fk"
        }
        fn options(&self) -> &'static [OptionSpec] {
            &[]
        }
        fn skip(&self, _: &Path, _: &LangOptions) -> bool {
            false
        }
        fn first_line_deeper_than(
            &self,
            _: &str,
            _: &Path,
            _: &LangOptions,
            _: usize,
        ) -> Option<u32> {
            None
        }
        fn analyze(&self, _: &str, _: &Path, _: &LangOptions) -> Result<Vec<Func>, ParseError> {
            Ok(vec![in_process_func()])
        }
    }
    static FAKE: Fake = Fake;

    fn in_process_func() -> Func {
        Func {
            name: "in-process".into(),
            kind: Kind {
                id: "fn",
                label: "fn",
            },
            line: 1,
            complexity: 1,
            depth: 0,
            points: vec![],
        }
    }

    /// Answers every request with one function `f`, complexity 2, unless the
    /// request contains `CRASH`, which aborts it.
    const ECHO: &str = r#"
        while IFS= read -r line; do
            case "$line" in *CRASH*) echo "boom" >&2; kill -ABRT $$;; esac
            id=${line#\{\"id\":}; id=${id%%,*}
            printf '{"id":%s,"functions":[{"name":"f","kind":{"id":"fn","label":"fn"},"line":1,"complexity":2,"depth":0,"points":[{"line":1,"branch":"if"}]}]}\n' "$id"
        done
    "#;

    /// Prints what the Rust runtime prints on a stack overflow, then aborts.
    const OVERFLOW: &str = r#"
        read -r line
        echo "thread 'parse' (1) has overflowed its stack" >&2
        echo "fatal runtime error: stack overflow, aborting" >&2
        kill -ABRT $$
    "#;

    fn fake(script: &str) -> SupervisorConfig {
        SupervisorConfig {
            command: Some(vec!["sh".into(), "-c".into(), script.into()]),
            timeout: Duration::from_secs(5),
            restarts: RestartLimit::PerRun { max: 3 },
        }
    }

    fn score(supervisor: &mut Supervisor, text: &str) -> Result<Vec<Func>, ParseError> {
        supervisor.analyze(&FAKE, text, Path::new("a.fk"), &LangOptions::default())
    }

    fn reason(result: Result<Vec<Func>, ParseError>) -> CrashReason {
        match result {
            Err(ParseError {
                kind: ParseErrorKind::Crashed { reason },
                ..
            }) => reason,
            other => panic!("expected a crash, got {other:?}"),
        }
    }

    #[test]
    fn a_healthy_worker_round_trips_and_is_reused() {
        let mut supervisor = Supervisor::new(fake(ECHO));
        for _ in 0..3 {
            let funcs = score(&mut supervisor, "x").unwrap();
            assert_eq!(funcs.len(), 1);
            assert_eq!((funcs[0].name.as_str(), funcs[0].complexity), ("f", 2));
            assert_eq!(
                funcs[0].points,
                vec![crate::model::Point {
                    line: 1,
                    branch: "if"
                }]
            );
        }
        assert_eq!(supervisor.spawned(), 1, "one worker serves every request");
        assert!(supervisor.take_warnings().is_empty());
    }

    #[test]
    fn an_abort_with_the_overflow_text_is_a_stack_overflow() {
        let mut supervisor = Supervisor::new(fake(OVERFLOW));
        let result = score(&mut supervisor, "x");
        assert_eq!(
            result.as_ref().unwrap_err().message,
            "not scored: the Fake analyzer crashed (stack overflow); \
             the file is probably nested too deeply to analyze"
        );
        assert_eq!(result.as_ref().unwrap_err().line, 0);
        assert_eq!(reason(result), CrashReason::StackOverflow);
    }

    #[cfg(unix)]
    #[test]
    fn an_abort_without_the_overflow_text_exits_with_the_first_stderr_line() {
        let script = "read -r line; printf '\\n   \\n  it broke  \\nsecond\\n' >&2; kill -ABRT $$";
        let result = score(&mut Supervisor::new(fake(script)), "x");
        assert_eq!(
            result.as_ref().unwrap_err().message,
            "not scored: the Fake analyzer crashed (signal SIGABRT): it broke"
        );
        assert_eq!(
            reason(result),
            CrashReason::Exited {
                termination: Termination::Signal(6),
                stderr: "it broke".into()
            }
        );
    }

    #[test]
    fn an_exit_code_is_reported_and_a_long_stderr_line_is_cut_to_200_chars() {
        let script = format!("read -r line; echo '{}' >&2; exit 3", "e".repeat(300));
        let result = score(&mut Supervisor::new(fake(&script)), "x");
        assert_eq!(
            result.unwrap_err().message,
            format!(
                "not scored: the Fake analyzer crashed (exit code 3): {}",
                "e".repeat(200)
            )
        );
        let silent = score(&mut Supervisor::new(fake("read -r line; exit 4")), "x");
        assert_eq!(
            silent.unwrap_err().message,
            "not scored: the Fake analyzer crashed (exit code 4)"
        );
    }

    #[test]
    fn a_hung_worker_times_out_and_is_killed() {
        let config = SupervisorConfig {
            timeout: Duration::from_millis(300),
            ..fake("exec sleep 30")
        };
        let mut supervisor = Supervisor::new(config);
        let started = Instant::now();
        let result = score(&mut supervisor, "x");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(
            result.as_ref().unwrap_err().message,
            "not scored: the Fake analyzer took longer than 300 ms"
        );
        assert_eq!(reason(result), CrashReason::Timeout);
    }

    #[test]
    fn a_malformed_response_is_a_crash_and_the_next_source_gets_a_fresh_worker() {
        let script = "read -r line; echo 'not json'; exec sleep 30";
        let mut supervisor = Supervisor::new(fake(script));
        let started = Instant::now();
        let result = score(&mut supervisor, "x");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the worker is killed, not awaited"
        );
        assert!(
            result.as_ref().unwrap_err().message.starts_with(
                "not scored: the Fake analyzer sent a bad response: response is not JSON"
            ),
            "{result:?}"
        );
        assert!(matches!(reason(result), CrashReason::BadResponse { .. }));
        assert!(score(&mut supervisor, "x").is_err());
        assert_eq!(supervisor.spawned(), 2);
    }

    #[test]
    fn a_refused_request_fails_its_source_and_keeps_the_worker() {
        let script = r#"
            while IFS= read -r line; do
                id=${line#\{\"id\":}; id=${id%%,*}
                printf '{"id":%s,"error":{"kind":"invalid","line":0,"message":"text larger than 30 MB"}}\n' "$id"
            done
        "#;
        let mut supervisor = Supervisor::new(fake(script));
        for _ in 0..2 {
            let error = score(&mut supervisor, "x").unwrap_err();
            assert_eq!(error.message, "not scored: text larger than 30 MB");
            assert_eq!(error.kind, ParseErrorKind::Refused);
        }
        assert_eq!(supervisor.spawned(), 1, "a refusal is not a crash");
    }

    #[test]
    fn after_a_crash_the_next_source_starts_a_fresh_worker() {
        let mut supervisor = Supervisor::new(fake(ECHO));
        assert!(score(&mut supervisor, "ok").is_ok());
        assert!(matches!(
            reason(score(&mut supervisor, "CRASH")),
            CrashReason::Exited { .. }
        ));
        assert_eq!(score(&mut supervisor, "ok").unwrap()[0].name, "f");
        assert_eq!(supervisor.spawned(), 2);
    }

    #[test]
    fn past_the_restart_cap_every_source_gives_up_without_a_restart() {
        let mut supervisor = Supervisor::new(fake(OVERFLOW));
        for _ in 0..4 {
            assert_eq!(
                reason(score(&mut supervisor, "x")),
                CrashReason::StackOverflow
            );
        }
        let result = score(&mut supervisor, "x");
        assert_eq!(
            result.as_ref().unwrap_err().message,
            "not scored: the analyzer kept crashing; gave up after 3 restarts"
        );
        assert_eq!(reason(result), CrashReason::GaveUp);
        assert_eq!(reason(score(&mut supervisor, "x")), CrashReason::GaveUp);
        assert_eq!(
            supervisor.spawned(),
            4,
            "the first worker and three restarts"
        );
    }

    #[test]
    fn a_rate_limit_pauses_restarts_until_the_window_passes() {
        let config = SupervisorConfig {
            restarts: RestartLimit::PerWindow {
                max: 1,
                window: Duration::from_millis(400),
            },
            ..fake(OVERFLOW)
        };
        let mut supervisor = Supervisor::new(config);
        assert_eq!(
            reason(score(&mut supervisor, "x")),
            CrashReason::StackOverflow
        );
        assert_eq!(
            reason(score(&mut supervisor, "x")),
            CrashReason::StackOverflow
        );
        let paused = score(&mut supervisor, "x");
        assert_eq!(
            paused.as_ref().unwrap_err().message,
            "not scored: the analyzer kept crashing; restarting is paused for 400 ms"
        );
        assert_eq!(reason(paused), CrashReason::GaveUp);
        thread::sleep(Duration::from_millis(450));
        assert_eq!(
            reason(score(&mut supervisor, "x")),
            CrashReason::StackOverflow
        );
        assert_eq!(supervisor.spawned(), 3);
    }

    #[test]
    fn a_worker_that_cannot_be_spawned_falls_back_to_in_process_and_warns_once() {
        let config = SupervisorConfig {
            command: Some(vec!["/nonexistent/cmplx-worker".into()]),
            ..fake("")
        };
        let mut supervisor = Supervisor::new(config);
        assert_eq!(score(&mut supervisor, "x"), Ok(vec![in_process_func()]));
        assert_eq!(score(&mut supervisor, "x"), Ok(vec![in_process_func()]));
        let warnings = supervisor.take_warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with("running without crash protection: "),
            "{}",
            warnings[0]
        );
        assert_eq!(supervisor.spawned(), 0);
    }

    /// A worker that deletes its own executable, then crashes: the restart
    /// after it cannot be spawned.
    #[cfg(unix)]
    fn self_deleting_worker() -> SupervisorConfig {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cmplx-supervisor-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("worker.sh");
        std::fs::write(&exe, "#!/bin/sh\nrm -f \"$0\"\nread -r line\nexit 3\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        SupervisorConfig {
            command: Some(vec![exe.into_os_string()]),
            ..fake("")
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_restart_fails_the_source_and_keeps_protection_on() {
        let mut supervisor = Supervisor::new(self_deleting_worker());
        assert!(matches!(reason(score(&mut supervisor, "x")), CrashReason::Exited { .. }));
        let error = score(&mut supervisor, "x").unwrap_err();
        assert!(
            error
                .message
                .starts_with("not scored: the Fake analyzer could not be restarted: "),
            "{}",
            error.message
        );
        assert!(matches!(reason(Err(error)), CrashReason::SpawnFailed { .. }));
        assert!(supervisor.take_warnings().is_empty(), "no fallback to in-process");
        for _ in 0..2 {
            assert!(matches!(reason(score(&mut supervisor, "x")), CrashReason::SpawnFailed { .. }));
        }
        assert_eq!(
            reason(score(&mut supervisor, "x")),
            CrashReason::GaveUp,
            "failed restarts count against the cap of 3"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dropping_the_supervisor_kills_and_reaps_its_worker() {
        let mut supervisor = Supervisor::new(fake(ECHO));
        assert!(score(&mut supervisor, "x").is_ok());
        let pid = supervisor.worker.as_ref().unwrap().child.id();
        drop(supervisor);
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!alive.success(), "worker {pid} outlived its supervisor");
    }

    #[test]
    fn the_production_limits() {
        assert_eq!(WORKER_TIMEOUT, Duration::from_secs(10));
        assert_eq!(MAX_WORKER_RESTARTS, 10);
        assert_eq!(MAX_WORKER_STDERR, 64 * 1024);
        assert_eq!(
            crash_message("Rust", &CrashReason::Timeout, &SupervisorConfig::cli()),
            "not scored: the Rust analyzer took longer than 10 s"
        );
        assert_eq!(
            crash_message("Rust", &CrashReason::GaveUp, &SupervisorConfig::cli()),
            "not scored: the analyzer kept crashing; gave up after 10 restarts"
        );
        assert_eq!(
            crash_message("Rust", &CrashReason::GaveUp, &SupervisorConfig::lsp()),
            "not scored: the analyzer kept crashing; restarting is paused for a minute"
        );
    }

    #[test]
    fn stderr_is_kept_up_to_the_cap() {
        let mut kept = vec![b'a'; MAX_WORKER_STDERR - 2];
        keep_capped(&mut kept, b"xyz");
        assert_eq!(kept.len(), MAX_WORKER_STDERR);
        assert!(kept.ends_with(b"xy"));
        keep_capped(&mut kept, b"more");
        assert_eq!(kept.len(), MAX_WORKER_STDERR);
    }

    #[test]
    fn a_worker_flooding_stderr_is_still_read_and_reaped() {
        let script = format!(
            "read -r line; head -c {} /dev/zero | tr '\\0' 'x' >&2; exit 1",
            MAX_WORKER_STDERR * 4
        );
        let result = score(&mut Supervisor::new(fake(&script)), "x");
        let message = result.unwrap_err().message;
        assert_eq!(
            message,
            format!(
                "not scored: the Fake analyzer crashed (exit code 1): {}",
                "x".repeat(200)
            )
        );
    }
}
