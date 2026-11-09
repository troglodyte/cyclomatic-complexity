//! Worker mode: `<binary> --worker` scores sources for a supervisor (see
//! [`crate::supervisor`]) so that an analyzer crash ends the worker, not the run.
//!
//! The protocol is one JSON object per line each way, built and read with
//! `serde_json::Value` (no serde derive, which would pull a parser crate into
//! every alias):
//!
//! - request: `{"id": n, "lang": "<id>", "path": "<path>", "text": "<source>", "options": {"<key>": bool}}`
//! - response: `{"id": n, "functions": [...]}`, or
//!   `{"id": n, "error": {"kind": "syntax" | "too_deep" | "invalid", "line": n, "limit": n, "message": "..."}}`
//!
//! `invalid` is a request the worker refused (not JSON, an unknown language,
//! text over [`MAX_TEXT_BYTES`]); its `id` is `null` when the request had none
//! that could be read. A refusal fails that source only; the worker serves on.
//!
//! **stdout is the protocol.** Nothing in worker mode may print to it except
//! through [`serve`].

use crate::language::{analyze_bounded, Lang, LangOptions, Registry};
use crate::model::{Func, Kind, ParseError, ParseErrorKind, Point};
use crate::run::{size_label, MAX_FILE_BYTES};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{self, BufRead, Read, Write};
use std::path::Path;
use std::sync::{Mutex, PoisonError};

/// The hidden flag that makes a binary a worker. Not in `--help`.
pub const WORKER_FLAG: &str = "--worker";

/// Most a JSON string escape grows its text: one control byte becomes `\u00XX`.
const MAX_ESCAPE_GROWTH: u64 = 6;

/// Room for everything in a request line besides its escaped text.
const REQUEST_OVERHEAD_BYTES: u64 = 64 * 1024;

/// Longest decoded text scored. Sources are capped at [`MAX_FILE_BYTES`] as
/// read, then decoded lossily, and each invalid byte becomes U+FFFD, three
/// bytes; so a source under the cap can decode to up to three times it.
pub const MAX_TEXT_BYTES: u64 = MAX_FILE_BYTES * 3;

/// Longest request line read. A source of [`MAX_FILE_BYTES`] fits however it
/// escapes: a valid byte escapes to at most six (`\u0001`), and an invalid
/// one is sent as U+FFFD, three bytes, unescaped. Anything longer is refused
/// without being read into memory.
pub(crate) const MAX_REQUEST_BYTES: u64 =
    MAX_FILE_BYTES * MAX_ESCAPE_GROWTH + REQUEST_OVERHEAD_BYTES;

/// Most distinct kind ids, kind labels and branch names a supervisor accepts
/// from its workers. Each is kept for the life of the process (the model
/// holds `&'static str`), so a worker cannot grow memory without bound.
const MAX_INTERNED: usize = 4096;

/// One source to score, as a supervisor sends it.
pub struct Request<'a> {
    pub id: u64,
    pub lang: &'a str,
    pub path: &'a Path,
    pub text: &'a str,
    pub opts: &'a LangOptions,
}

/// One line read with a cap on its length.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Line {
    Eof,
    /// The line without its `\n`.
    Text(Vec<u8>),
    /// Longer than the cap; only the cap and one more byte were read.
    TooLong,
}

/// Read one `\n`-terminated line of at most `max` bytes (without the `\n`).
/// A last line without `\n` counts. A longer line is not read past `max + 1`
/// bytes, so the stream is then out of step and must be abandoned.
pub(crate) fn read_line_capped(input: &mut impl BufRead, max: u64) -> io::Result<Line> {
    let mut bytes = Vec::new();
    let read = Read::take(&mut *input, max.saturating_add(1)).read_until(b'\n', &mut bytes)?;
    if read == 0 {
        return Ok(Line::Eof);
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.len() as u64 > max {
        return Ok(Line::TooLong);
    }
    Ok(Line::Text(bytes))
}

/// The request as one line, `\n` included.
pub fn encode_request(request: &Request) -> String {
    let options: serde_json::Map<String, Value> = request
        .opts
        .iter()
        .map(|(key, value)| (key.to_string(), Value::Bool(value)))
        .collect();
    let line = json!({
        "id": request.id,
        "lang": request.lang,
        "path": request.path.to_string_lossy(),
        "text": request.text,
        "options": options,
    });
    format!("{line}\n")
}

/// Serve requests from `input` until it ends, scoring each with the language
/// of `reg` it names. The loop is bounded by the input: one response per
/// request line. A line over [`MAX_REQUEST_BYTES`] is refused and ends the
/// loop, since the rest of it cannot be told from the next request.
pub fn serve(reg: &Registry, input: &mut impl BufRead, out: &mut impl Write) -> io::Result<()> {
    serve_with_cap(reg, input, out, MAX_TEXT_BYTES)
}

/// [`serve`], refusing texts over `max_text` bytes rather than [`MAX_TEXT_BYTES`].
fn serve_with_cap(
    reg: &Registry,
    input: &mut impl BufRead,
    out: &mut impl Write,
    max_text: u64,
) -> io::Result<()> {
    loop {
        let response = match read_line_capped(input, MAX_REQUEST_BYTES)? {
            Line::Eof => return Ok(()),
            Line::Text(line) => respond(reg, &line, max_text),
            Line::TooLong => {
                let message = format!("request longer than {} bytes", MAX_REQUEST_BYTES);
                return write_line(out, &invalid(Value::Null, &message));
            }
        };
        write_line(out, &response)?;
    }
}

fn write_line(out: &mut impl Write, response: &Value) -> io::Result<()> {
    writeln!(out, "{response}")?;
    out.flush()
}

/// The response to one request line.
fn respond(reg: &Registry, line: &[u8], max_text: u64) -> Value {
    let request: Value = match serde_json::from_slice(line) {
        Ok(request) => request,
        Err(e) => return invalid(Value::Null, &format!("request is not JSON: {e}")),
    };
    let (id, lang, path, text, opts) = match parse_request(reg, &request, max_text) {
        Ok(parsed) => parsed,
        Err(refusal) => return refusal,
    };
    match analyze_bounded(lang, text, Path::new(path), &opts) {
        Ok(funcs) => {
            json!({ "id": id, "functions": funcs.iter().map(func_json).collect::<Vec<_>>() })
        }
        Err(e) => json!({ "id": id, "error": error_json(&e) }),
    }
}

/// A usable request's id, language, path, text and options. `Err` is the
/// `invalid` response for one that is not usable.
fn parse_request<'a>(
    reg: &Registry,
    request: &'a Value,
    max_text: u64,
) -> Result<(u64, Lang, &'a str, &'a str, LangOptions), Value> {
    let id = request["id"]
        .as_u64()
        .ok_or_else(|| invalid(Value::Null, "request has no numeric id"))?;
    let lang_id = request["lang"].as_str().unwrap_or_default();
    let lang = reg
        .by_id(lang_id)
        .ok_or_else(|| invalid(json!(id), &format!("unknown language `{lang_id}`")))?;
    let (Some(path), Some(text)) = (request["path"].as_str(), request["text"].as_str()) else {
        return Err(invalid(json!(id), "request has no text or path"));
    };
    if text.len() as u64 > max_text {
        return Err(invalid(
            json!(id),
            &format!("text larger than {}", size_label(max_text)),
        ));
    }
    let mut opts = LangOptions::default();
    for spec in lang.options() {
        if let Some(value) = request["options"][spec.key].as_bool() {
            opts.set(spec.key, value);
        }
    }
    Ok((id, lang, path, text, opts))
}

fn invalid(id: Value, message: &str) -> Value {
    json!({ "id": id, "error": { "kind": "invalid", "line": 0, "message": message } })
}

fn func_json(f: &Func) -> Value {
    let points: Vec<Value> = f
        .points
        .iter()
        .map(|p| json!({ "line": p.line, "branch": p.branch }))
        .collect();
    json!({
        "name": f.name,
        "kind": { "id": f.kind.id, "label": f.kind.label },
        "line": f.line,
        "complexity": f.complexity,
        "depth": f.depth,
        "points": points,
    })
}

fn error_json(e: &ParseError) -> Value {
    match &e.kind {
        ParseErrorKind::Syntax => json!({ "kind": "syntax", "line": e.line, "message": e.message }),
        ParseErrorKind::TooDeep { limit } => {
            json!({ "kind": "too_deep", "line": e.line, "limit": limit, "message": e.message })
        }
        // `analyze_bounded` never returns these; say so rather than guess.
        ParseErrorKind::Crashed { .. } | ParseErrorKind::Refused => {
            json!({ "kind": "invalid", "line": 0, "message": e.message })
        }
    }
}

/// Read the response to request `id`. `Err` describes a line that is not one.
pub fn decode_response(line: &[u8], id: u64) -> Result<Result<Vec<Func>, ParseError>, String> {
    let response: Value =
        serde_json::from_slice(line).map_err(|e| format!("response is not JSON: {e}"))?;
    if response["id"].as_u64() != Some(id) {
        return Err(format!(
            "expected a response to request {id}, got id {}",
            response["id"]
        ));
    }
    if let Some(error) = response.get("error") {
        return decode_error(error).map(Err);
    }
    let functions = response["functions"]
        .as_array()
        .ok_or("response has neither functions nor an error")?;
    functions
        .iter()
        .map(decode_func)
        .collect::<Result<_, _>>()
        .map(Ok)
}

fn decode_error(error: &Value) -> Result<ParseError, String> {
    let line = number(&error["line"]).ok_or("error has no line")?;
    let message = error["message"]
        .as_str()
        .ok_or("error has no message")?
        .to_string();
    let kind = match error["kind"].as_str() {
        Some("syntax") => ParseErrorKind::Syntax,
        Some("too_deep") => {
            let limit = error["limit"]
                .as_u64()
                .ok_or("too_deep error has no limit")?;
            ParseErrorKind::TooDeep {
                limit: usize::try_from(limit).map_err(|e| e.to_string())?,
            }
        }
        // The worker refused this source and is still serving: the source
        // fails, the worker is fine.
        Some("invalid") => {
            return Ok(ParseError {
                line: 0,
                message: format!("not scored: {message}"),
                kind: ParseErrorKind::Refused,
            })
        }
        other => return Err(format!("unknown error kind {other:?}")),
    };
    Ok(ParseError {
        line,
        message,
        kind,
    })
}

fn decode_func(f: &Value) -> Result<Func, String> {
    let points = f["points"]
        .as_array()
        .ok_or("function has no points")?
        .iter()
        .map(|p| {
            Ok(Point {
                line: number(&p["line"]).ok_or("point has no line")?,
                branch: intern(p["branch"].as_str().ok_or("point has no branch")?)?,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(Func {
        name: f["name"]
            .as_str()
            .ok_or("function has no name")?
            .to_string(),
        kind: Kind {
            id: intern(f["kind"]["id"].as_str().ok_or("function has no kind id")?)?,
            label: intern(
                f["kind"]["label"]
                    .as_str()
                    .ok_or("function has no kind label")?,
            )?,
        },
        line: number(&f["line"]).ok_or("function has no line")?,
        complexity: number(&f["complexity"]).ok_or("function has no complexity")?,
        depth: number(&f["depth"]).ok_or("function has no depth")?,
        points,
    })
}

fn number(v: &Value) -> Option<u32> {
    v.as_u64().and_then(|n| u32::try_from(n).ok())
}

/// `s` as a `&'static str`, leaked once per distinct value and at most
/// [`MAX_INTERNED`] of them.
fn intern(s: &str) -> Result<&'static str, String> {
    static INTERNED: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let mut interned = INTERNED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(&kept) = interned.get(s) {
        return Ok(kept);
    }
    if interned.len() >= MAX_INTERNED {
        return Err(format!("more than {MAX_INTERNED} distinct names"));
    }
    let kept: &'static str = Box::leak(s.to_string().into_boxed_str());
    interned.insert(kept);
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::{Language, OptionSpec};

    const SHOUT: OptionSpec = OptionSpec {
        key: "shout",
        on_flag: None,
        off_flag: None,
        default: Some(false),
        help: "",
    };
    static OPTIONS: [OptionSpec; 1] = [SHOUT];

    /// Each line `name score` is a function with one `if` point; a line `!`
    /// is a syntax error; a line of more `(` than the cap is too deep. With
    /// `shout`, names are upper-cased. The path's extension is appended to
    /// each name so the path is observable.
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
            &OPTIONS
        }
        fn skip(&self, _: &Path, _: &LangOptions) -> bool {
            false
        }
        fn first_line_deeper_than(
            &self,
            src: &str,
            _: &Path,
            _: &LangOptions,
            limit: usize,
        ) -> Option<u32> {
            src.lines()
                .position(|line| line.chars().filter(|&c| c == '(').count() > limit)
                .map(|i| i as u32 + 1)
        }
        fn analyze(
            &self,
            src: &str,
            path: &Path,
            opts: &LangOptions,
        ) -> Result<Vec<Func>, ParseError> {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            src.lines()
                .enumerate()
                .map(|(i, line)| {
                    let line_no = i as u32 + 1;
                    let (name, cc) = line.split_once(' ').ok_or(ParseError {
                        line: line_no,
                        message: "bad".into(),
                        kind: ParseErrorKind::Syntax,
                    })?;
                    let name = if opts.flag(&SHOUT) == Some(true) {
                        name.to_uppercase()
                    } else {
                        name.to_string()
                    };
                    Ok(Func {
                        name: format!("{name}.{ext}"),
                        kind: Kind {
                            id: "trait_fn",
                            label: "trait fn",
                        },
                        line: line_no,
                        complexity: cc.parse().unwrap_or(1),
                        depth: 1,
                        points: vec![Point {
                            line: line_no,
                            branch: "if",
                        }],
                    })
                })
                .collect()
        }
    }
    static FAKE: Fake = Fake;

    fn reg() -> Registry {
        Registry::alias(&FAKE)
    }

    /// Serve `input` and return each response line, parsed.
    fn served(input: &str) -> Vec<Value> {
        let mut out = Vec::new();
        serve(&reg(), &mut input.as_bytes(), &mut out).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn request(id: u64, text: &str, opts: &LangOptions) -> String {
        encode_request(&Request {
            id,
            lang: "fake",
            path: Path::new("dir/a.fk"),
            text,
            opts,
        })
    }

    #[test]
    fn a_request_is_one_json_line_with_every_field() {
        let mut opts = LangOptions::default();
        opts.set("shout", true);
        let line = request(7, "f 2\n", &opts);
        assert!(
            line.ends_with('\n') && line.matches('\n').count() == 1,
            "{line:?}"
        );
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            v,
            json!({ "id": 7, "lang": "fake", "path": "dir/a.fk", "text": "f 2\n", "options": { "shout": true } })
        );
    }

    #[test]
    fn a_request_round_trips_through_the_worker() {
        let mut opts = LangOptions::default();
        opts.set("shout", true);
        let input = request(3, "f 2\ng 1\n", &opts);
        let mut out = Vec::new();
        serve(&reg(), &mut input.as_bytes(), &mut out).unwrap();
        let funcs = decode_response(out.strip_suffix(b"\n").unwrap(), 3)
            .unwrap()
            .unwrap();
        assert_eq!(
            funcs,
            FAKE.analyze("f 2\ng 1\n", Path::new("dir/a.fk"), &opts)
                .unwrap(),
            "the worker scores exactly as in process, path and options included"
        );
        assert_eq!(funcs[0].name, "F.fk");
    }

    #[test]
    fn errors_round_trip_with_their_kind() {
        let syntax = request(1, "f 1\n!\n", &LangOptions::default());
        let deep = request(
            2,
            &format!("f 1\n{}\n", "(".repeat(101)),
            &LangOptions::default(),
        );
        let mut out = Vec::new();
        serve(&reg(), &mut format!("{syntax}{deep}").as_bytes(), &mut out).unwrap();
        let lines: Vec<&[u8]> = out
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(
            decode_response(lines[0], 1).unwrap(),
            Err(ParseError {
                line: 2,
                message: "bad".into(),
                kind: ParseErrorKind::Syntax
            })
        );
        assert_eq!(
            decode_response(lines[1], 2).unwrap(),
            Err(ParseError {
                line: 2,
                message: "nesting deeper than 100 levels; this code needs restructuring".into(),
                kind: ParseErrorKind::TooDeep { limit: 100 },
            })
        );
    }

    #[test]
    fn a_request_the_worker_cannot_use_gets_an_invalid_error_and_the_next_is_served() {
        let good = request(5, "f 1\n", &LangOptions::default());
        let responses = served(&format!(
            "not json\n{{\"id\":4,\"lang\":\"cobol\",\"path\":\"a\",\"text\":\"\"}}\n{good}"
        ));
        assert_eq!(responses.len(), 3, "{responses:?}");
        assert_eq!(responses[0]["id"], Value::Null);
        assert_eq!(responses[0]["error"]["kind"], "invalid");
        assert_eq!(responses[1]["id"], 4);
        assert_eq!(responses[1]["error"]["message"], "unknown language `cobol`");
        assert_eq!(responses[2]["id"], 5);
        assert!(responses[2]["functions"].is_array());
    }

    #[test]
    fn decoded_text_is_capped_at_three_times_the_file_cap() {
        assert_eq!(MAX_TEXT_BYTES, MAX_FILE_BYTES * 3);
        // A file of invalid bytes decodes to three bytes (U+FFFD) per byte, so
        // a file at the cap must not be refused once decoded.
        let at_cap = format!("f 1\n{}", "\u{FFFD}".repeat(4));
        assert_eq!(at_cap.len(), 16);
        let input = request(8, &at_cap, &LangOptions::default());
        let mut out = Vec::new();
        serve_with_cap(&reg(), &mut input.as_bytes(), &mut out, 16).unwrap();
        let response: Value = serde_json::from_slice(&out).unwrap();
        assert_ne!(response["error"]["kind"], "invalid", "{response}");

        let mut out = Vec::new();
        serve_with_cap(&reg(), &mut input.as_bytes(), &mut out, 15).unwrap();
        let response: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(response["id"], 8);
        assert_eq!(response["error"]["kind"], "invalid");
        assert_eq!(response["error"]["message"], "text larger than 15 bytes");
    }

    #[test]
    fn a_line_over_the_request_cap_is_refused_unread_and_ends_the_worker() {
        let mut input = io::Cursor::new(vec![b'x'; 64]);
        assert_eq!(read_line_capped(&mut input, 10).unwrap(), Line::TooLong);
        assert_eq!(
            input.position(),
            11,
            "read no further than the cap and one byte"
        );

        let mut capped = io::Cursor::new(b"0123456789\nab".to_vec());
        assert_eq!(
            read_line_capped(&mut capped, 10).unwrap(),
            Line::Text(b"0123456789".to_vec())
        );
        assert_eq!(
            read_line_capped(&mut capped, 10).unwrap(),
            Line::Text(b"ab".to_vec())
        );
        assert_eq!(read_line_capped(&mut capped, 10).unwrap(), Line::Eof);
    }

    #[test]
    fn the_request_cap_fits_a_file_at_the_cap_however_it_escapes() {
        assert_eq!(MAX_REQUEST_BYTES, MAX_FILE_BYTES * 6 + 64 * 1024);
        let worst = request(1, "\u{1}", &LangOptions::default());
        assert!(
            worst.contains("\\u0001"),
            "one byte escapes to six: {worst}"
        );
    }

    #[test]
    fn a_response_to_another_request_or_of_the_wrong_shape_is_rejected() {
        assert!(decode_response(br#"{"id":2,"functions":[]}"#, 1)
            .unwrap_err()
            .contains("request 1"));
        assert!(decode_response(b"nope", 1).is_err());
        assert!(decode_response(br#"{"id":1}"#, 1).is_err());
        assert!(decode_response(br#"{"id":1,"functions":[{"name":"f"}]}"#, 1).is_err());
        assert!(decode_response(
            br#"{"id":1,"error":{"kind":"odd","line":1,"message":"m"}}"#,
            1
        )
        .is_err());
        let refused = decode_response(
            br#"{"id":1,"error":{"kind":"invalid","line":0,"message":"m"}}"#,
            1,
        );
        assert_eq!(
            refused,
            Ok(Err(ParseError {
                line: 0,
                message: "not scored: m".into(),
                kind: ParseErrorKind::Refused,
            })),
            "a refusal is the source's failure, not the worker's"
        );
    }

    #[test]
    fn interned_names_are_shared() {
        let a = intern("worker-test-kind").unwrap();
        let b = intern(&String::from("worker-test-kind")).unwrap();
        assert!(std::ptr::eq(a, b), "one copy per distinct name");
    }
}
