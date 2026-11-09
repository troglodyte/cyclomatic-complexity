//! A bound on bracket nesting, checked before `syn` parses.
//!
//! `syn` is a recursive-descent parser and the analyzer's walk recurses with
//! it, so stack use grows with nesting depth. A stack overflow aborts the
//! process (`catch_unwind` cannot catch it), which would end a CLI run and the
//! shared language server. So core refuses a file whose `(`/`[`/`{` nesting
//! goes deeper than its cap before `syn` sees it, using this scan.
//!
//! The depth is counted on proc-macro2's token tree. Its lexer keeps an explicit
//! stack (`parse::token_stream` in proc-macro2 1.0.107) and its `TokenStream`
//! drop is non-recursive, so lexing a deep file is safe; the tree is then
//! walked with an explicit stack here. Brackets inside strings, chars and
//! comments are not tokens, so they do not count.

use proc_macro2::{token_stream, TokenStream, TokenTree};

// Measured with syn 3.0.9 (`syn::parse_file` + the analyzer's walk + drop, one
// file per process, on a thread of the given stack): the deepest delimiter
// nesting that still succeeded, by what each level nests.
//
// | each level      | debug 2 MB | debug 16 MB | release 2 MB | release 16 MB |
// |-----------------|-----------:|------------:|-------------:|--------------:|
// | `(`             |        142 |       1,168 |          470 |         3,758 |
// | `if a {`        |         76 |         623 |          293 |         2,341 |
// | `g(|| {` (2)    |         63 |         515 |          225 |         1,799 |
// | `|| {`          |         43 |         343 |          161 |         1,284 |
//
// Core's cap is 100 levels. The parse runs on its own 16 MB thread
// (`PARSE_STACK_BYTES` in `lib.rs`), where the worst debug figure is 343.

/// The 1-based line on which bracket nesting first goes deeper than `limit`;
/// `None` if it never does.
///
/// Mirrors `syn::parse_file`'s prologue: a BOM is ignored, and a `#!` first
/// line may or may not be a shebang that `syn` drops, so both readings are
/// checked. A text that does not lex is not too deep: `syn` lexes it the same
/// way and fails before parsing anything.
pub fn first_line_deeper_than(src: &str, limit: usize) -> Option<u32> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let without_first_line = src
        .starts_with("#!")
        .then(|| src.find('\n').map_or("", |newline| &src[newline..]));
    first_too_deep(src, limit).or_else(|| without_first_line.and_then(|rest| first_too_deep(rest, limit)))
}

/// [`first_line_deeper_than`] for one reading of the text: one pass over the
/// token tree with an explicit stack of groups, stopping at the first group
/// opened past `limit`.
fn first_too_deep(text: &str, limit: usize) -> Option<u32> {
    let tokens: TokenStream = text.parse().ok()?;
    let mut stack: Vec<token_stream::IntoIter> = vec![tokens.into_iter()];
    while let Some(frame) = stack.last_mut() {
        let Some(token) = frame.next() else {
            stack.pop();
            continue;
        };
        if let TokenTree::Group(group) = token {
            // The stack holds the top level plus every open group.
            if stack.len() > limit {
                return Some(group.span_open().start().line as u32);
            }
            stack.push(group.stream().into_iter());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_NESTING: usize = 100;

    fn line_of(src: &str) -> Option<u32> {
        first_line_deeper_than(src, MAX_NESTING)
    }

    fn parens(depth: usize) -> String {
        format!("{}{}", "(".repeat(depth), ")".repeat(depth))
    }

    #[test]
    fn depth_at_the_cap_passes_and_one_more_is_reported() {
        assert_eq!(line_of(&parens(MAX_NESTING)), None);
        assert_eq!(
            line_of(&format!("\n{}", parens(MAX_NESTING + 1))),
            Some(2)
        );
    }

    #[test]
    fn the_first_excess_in_source_order_is_the_one_reported() {
        let src = format!(
            "{}\n{}\n{}",
            parens(1),
            parens(MAX_NESTING + 1),
            parens(MAX_NESTING + 5)
        );
        assert_eq!(line_of(&src), Some(2));
    }

    #[test]
    fn all_three_delimiters_count() {
        let open = "([{".repeat(MAX_NESTING / 3 + 1);
        let close: String = open
            .chars()
            .rev()
            .map(|c| match c {
                '(' => ')',
                '[' => ']',
                _ => '}',
            })
            .collect();
        assert_eq!(line_of(&format!("{open}{close}")), Some(1));
        assert_eq!(line_of("([{}])"), None);
    }

    #[test]
    fn a_text_that_does_not_lex_is_left_to_syn() {
        assert_eq!(line_of(&"(".repeat(MAX_NESTING * 2)), None);
    }

    #[test]
    fn a_bom_is_ignored() {
        let src = format!("\u{feff}{}", parens(MAX_NESTING + 1));
        assert_eq!(line_of(&src), Some(1));
    }

    #[test]
    fn a_shebang_line_that_does_not_lex_does_not_hide_the_rest() {
        let src = format!("#!/bin/sh '\n{}", parens(MAX_NESTING + 1));
        assert_eq!(line_of(&src), Some(2));
    }
}
