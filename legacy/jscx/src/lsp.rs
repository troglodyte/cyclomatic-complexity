use crate::analyze::{analyze, Options};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Read, Write};

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcMessage {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<serde_json::Value>,
    // A JSON-RPC Response must not carry a `method` member at all, so this is
    // omitted rather than serialized as null.
    #[serde(skip_serializing_if = "Option::is_none")]
    method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
}

/// Score at which a function starts warning. Twice this value is an error.
pub const DEFAULT_THRESHOLD: u32 = 11;

/// Upper bound on a JSON-RPC body, so a bogus `Content-Length` cannot ask us to
/// allocate an arbitrary amount of memory.
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

pub fn run() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut stdout = io::stdout();

    let mut threshold = DEFAULT_THRESHOLD;

    loop {
        let mut line = String::new();
        stdin.read_line(&mut line)?;
        if line.is_empty() {
            break;
        }

        // Anything that is not a usable Content-Length header is skipped rather
        // than fatal: a malformed frame must not take the server down.
        let Some(len) = parse_content_length(&line) else {
            continue;
        };

        // Read the rest of the headers until \r\n\r\n
        loop {
            line.clear();
            stdin.read_line(&mut line)?;
            if line.is_empty() || line == "\r\n" || line == "\n" {
                break;
            }
        }

        let mut body = vec![0u8; len];
        stdin.read_exact(&mut body)?;

        let Ok(msg) = serde_json::from_slice::<JsonRpcMessage>(&body) else {
            continue;
        };
        let Some(method) = &msg.method else { continue };

        match method.as_str() {
            "initialize" => {
                if let Some(params) = &msg.params {
                    threshold = threshold_from_params(params);
                }
                let result = serde_json::json!({
                    "capabilities": {
                        "textDocumentSync": 1, // Full sync
                    }
                });
                send_response(&mut stdout, msg.id, result)?;
            }
            "textDocument/didOpen" | "textDocument/didChange" | "textDocument/didSave" => {
                if let Some(params) = msg.params {
                    handle_notification(&mut stdout, method, params, threshold)?;
                }
            }
            "shutdown" => {
                send_response(&mut stdout, msg.id, serde_json::Value::Null)?;
            }
            "exit" => {
                return Ok(());
            }
            _ => {}
        }
    }

    Ok(())
}

/// Parse a `Content-Length` header value, rejecting anything unusable.
fn parse_content_length(line: &str) -> Option<usize> {
    let (name, value) = line.split_once(':')?;
    if !name.trim().eq_ignore_ascii_case("content-length") {
        return None;
    }
    let len: usize = value.trim().parse().ok()?;
    if len == 0 || len > MAX_BODY_BYTES {
        return None;
    }
    Some(len)
}

/// Read the warning threshold out of an `initialize` request's params.
fn threshold_from_params(params: &serde_json::Value) -> u32 {
    params
        .get("initializationOptions")
        .and_then(|opts| opts.get("threshold"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|t| u32::try_from(t).ok())
        .filter(|t| *t > 0)
        .unwrap_or(DEFAULT_THRESHOLD)
}

/// JSX lexing is on for every extension except plain TypeScript, matching the CLI.
fn is_jsx_uri(uri: &str) -> bool {
    !(uri.ends_with(".ts") || uri.ends_with(".mts") || uri.ends_with(".cts"))
}

/// Analyze one document and build its LSP diagnostics.
///
/// Functions scoring at or above `threshold` are warnings; at or above twice the
/// threshold they are errors. Declaration files are analyzed like any other
/// document — filtering them is the client's job, since only the client knows
/// which files it chose to open.
fn diagnostics_for(text: &str, uri: &str, threshold: u32) -> Vec<serde_json::Value> {
    // LSP character offsets are UTF-16 code units, not bytes or chars.
    let widths: Vec<u32> = text
        .lines()
        .map(|l| l.chars().map(char::len_utf16).sum::<usize>() as u32)
        .collect();

    analyze(text, is_jsx_uri(uri), Options::default())
        .iter()
        .filter(|f| f.complexity >= threshold)
        .map(|f| {
            let row = f.line.saturating_sub(1);
            let end = widths.get(row as usize).copied().unwrap_or(0);
            let severity = if f.complexity >= threshold.saturating_mul(2) {
                1 // Error
            } else {
                2 // Warning
            };
            serde_json::json!({
                "range": {
                    "start": { "line": row, "character": 0 },
                    "end": { "line": row, "character": end },
                },
                "severity": severity,
                "source": "jscx",
                "message": format!(
                    "Complexity is {} for {} '{}'",
                    f.complexity,
                    f.kind.as_str(),
                    f.name
                ),
            })
        })
        .collect()
}

fn send_response(stdout: &mut io::Stdout, id: Option<serde_json::Value>, result: serde_json::Value) -> io::Result<()> {
    let resp = JsonRpcMessage {
        jsonrpc: "2.0".to_string(),
        id,
        method: None,
        params: None,
        result: Some(result),
    };
    send_message(stdout, &resp)
}

fn send_message(stdout: &mut io::Stdout, msg: &JsonRpcMessage) -> io::Result<()> {
    let body = serde_json::to_string(msg)?;
    write!(stdout, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    stdout.flush()
}

fn handle_notification(stdout: &mut io::Stdout, method: &str, params: serde_json::Value, threshold: u32) -> io::Result<()> {
    let (uri, text) = match method {
        "textDocument/didOpen" | "textDocument/didSave" => {
            let uri = params["textDocument"]["uri"].as_str().unwrap().to_string();
            let text = params["textDocument"]["text"].as_str().map(|s| s.to_string());
            // For didSave, text might not be provided. We might need to read from disk or ignore.
            // But usually didOpen provides it.
            (uri, text)
        }
        "textDocument/didChange" => {
            let uri = params["textDocument"]["uri"].as_str().unwrap().to_string();
            let text = params["contentChanges"][0]["text"].as_str().map(|s| s.to_string());
            (uri, text)
        }
        _ => return Ok(()),
    };

    if let Some(text) = text {
        let diagnostics = diagnostics_for(&text, &uri, threshold);

        let notification = JsonRpcMessage {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: Some("textDocument/publishDiagnostics".to_string()),
            params: Some(serde_json::json!({
                "uri": uri,
                "diagnostics": diagnostics,
            })),
            result: None,
        };
        send_message(stdout, &notification)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A single-line function whose complexity is `1 + branches`.
    fn one_liner(branches: usize) -> String {
        format!("function f(a) {{{}}}", "if(a){}".repeat(branches))
    }

    fn utf16_len(s: &str) -> usize {
        s.chars().map(char::len_utf16).sum()
    }

    #[test]
    fn default_threshold_is_eleven() {
        assert_eq!(DEFAULT_THRESHOLD, 11);
    }

    #[test]
    fn threshold_falls_back_to_default_when_absent() {
        assert_eq!(threshold_from_params(&json!({})), DEFAULT_THRESHOLD);
        assert_eq!(
            threshold_from_params(&json!({ "initializationOptions": {} })),
            DEFAULT_THRESHOLD
        );
    }

    #[test]
    fn threshold_is_read_from_initialization_options() {
        let params = json!({ "initializationOptions": { "threshold": 25 } });
        assert_eq!(threshold_from_params(&params), 25);
    }

    #[test]
    fn threshold_rejects_zero_so_severity_bands_stay_meaningful() {
        let params = json!({ "initializationOptions": { "threshold": 0 } });
        assert_eq!(threshold_from_params(&params), DEFAULT_THRESHOLD);
    }

    #[test]
    fn score_below_threshold_is_not_reported() {
        let diags = diagnostics_for(&one_liner(9), "file:///a.ts", 11);
        assert!(diags.is_empty(), "complexity 10 should not warn at 11");
    }

    #[test]
    fn score_at_threshold_is_a_warning() {
        let diags = diagnostics_for(&one_liner(10), "file:///a.ts", 11);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0]["severity"], 2);
        assert_eq!(diags[0]["source"], "jscx");
    }

    #[test]
    fn score_at_double_threshold_is_an_error() {
        let diags = diagnostics_for(&one_liner(21), "file:///a.ts", 11);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0]["severity"], 1);
    }

    #[test]
    fn score_just_below_double_threshold_is_still_a_warning() {
        let diags = diagnostics_for(&one_liner(20), "file:///a.ts", 11);
        assert_eq!(diags[0]["severity"], 2);
    }

    #[test]
    fn range_ends_at_the_actual_line_length() {
        let src = one_liner(10);
        let diags = diagnostics_for(&src, "file:///a.ts", 11);
        assert_eq!(diags[0]["range"]["start"]["line"], 0);
        assert_eq!(diags[0]["range"]["start"]["character"], 0);
        assert_eq!(diags[0]["range"]["end"]["line"], 0);
        assert_eq!(diags[0]["range"]["end"]["character"], src.len());
    }

    #[test]
    fn range_end_counts_utf16_units_not_chars() {
        // U+1D518 is one char but two UTF-16 code units; LSP positions are UTF-16.
        let src = format!("function f(a) {{ const s = \"\u{1D518}\";{}}}", "if(a){}".repeat(10));
        let diags = diagnostics_for(&src, "file:///a.ts", 11);
        let expected = utf16_len(&src);
        assert_eq!(expected, src.chars().count() + 1);
        assert_eq!(diags[0]["range"]["end"]["character"], expected);
    }

    #[test]
    fn range_is_anchored_to_the_functions_own_line() {
        let src = format!("const x = 1;\n{}\n", one_liner(10));
        let diags = diagnostics_for(&src, "file:///a.ts", 11);
        assert_eq!(diags[0]["range"]["start"]["line"], 1);
        assert_eq!(diags[0]["range"]["end"]["character"], one_liner(10).len());
    }

    #[test]
    fn message_names_the_function_and_kind() {
        let diags = diagnostics_for(&one_liner(10), "file:///a.ts", 11);
        let msg = diags[0]["message"].as_str().unwrap();
        assert!(msg.contains("11"), "{msg}");
        assert!(msg.contains("function"), "{msg}");
        assert!(msg.contains('f'), "{msg}");
    }

    #[test]
    fn declaration_files_are_analyzed_because_filtering_is_the_clients_job() {
        let diags = diagnostics_for(&one_liner(10), "file:///a.d.ts", 11);
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn jsx_is_lexed_except_for_plain_typescript_extensions() {
        // In JSX mode the `&&` inside element text is text, not an operator.
        let src = "function f(a) { return <span>a && b</span>; }";
        let tsx = diagnostics_for(src, "file:///a.tsx", 1);
        assert!(tsx[0]["message"].as_str().unwrap().contains("is 1"));

        for ext in ["ts", "mts", "cts"] {
            let uri = format!("file:///a.{ext}");
            assert!(!is_jsx_uri(&uri), "{uri} should not use JSX lexing");
        }
        for ext in ["tsx", "js", "jsx", "mjs", "cjs"] {
            let uri = format!("file:///a.{ext}");
            assert!(is_jsx_uri(&uri), "{uri} should use JSX lexing");
        }
    }

    #[test]
    fn clean_documents_publish_an_empty_list_to_clear_stale_markers() {
        let diags = diagnostics_for("const x = 1;\n", "file:///a.ts", 11);
        assert!(diags.is_empty());
    }

    #[test]
    fn a_response_carries_no_method_member() {
        let resp = JsonRpcMessage {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: None,
            params: None,
            result: Some(json!({})),
        };
        let encoded = serde_json::to_string(&resp).unwrap();
        assert!(!encoded.contains("method"), "{encoded}");
    }

    #[test]
    fn content_length_parses_only_well_formed_headers() {
        assert_eq!(parse_content_length("Content-Length: 42\r\n"), Some(42));
        assert_eq!(parse_content_length("content-length: 42\r\n"), Some(42));
        assert_eq!(parse_content_length("Content-Length: notanumber\r\n"), None);
        assert_eq!(parse_content_length("Content-Length: \r\n"), None);
        assert_eq!(parse_content_length("Content-Type: utf-8\r\n"), None);
        assert_eq!(parse_content_length(""), None);
    }

    #[test]
    fn oversized_content_length_is_rejected_rather_than_allocated() {
        let header = format!("Content-Length: {}\r\n", u64::MAX);
        assert_eq!(parse_content_length(&header), None);
    }
}
