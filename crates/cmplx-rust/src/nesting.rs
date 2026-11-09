//! A bound on delimiter nesting, checked before `syn` parses.
//!
//! `syn` is a recursive-descent parser and the analyzer's walk recurses with
//! it, so stack use grows with nesting depth. A stack overflow aborts the
//! process (`catch_unwind` cannot catch it), which would end a CLI run and the
//! shared language server. Files nested deeper than [`MAX_NESTING`] are
//! therefore refused before `syn` sees them.
//!
//! The depth is counted on proc-macro2's token tree. Its lexer keeps an explicit
//! stack (`parse::token_stream` in proc-macro2 1.0.107) and its `TokenStream`
//! drop is non-recursive, so lexing a deep file is safe; the tree is then
//! walked with an explicit stack here. Brackets inside strings, chars and
//! comments are not tokens, so they do not count.

use proc_macro2::{TokenStream, TokenTree};

/// Deepest `(`/`[`/`{` nesting a Rust file may have and still be scored.
///
/// Measured with syn 3.0.9 (`syn::parse_file` + the analyzer's walk + drop,
/// one file per process, on a thread of the given stack): the deepest
/// delimiter nesting that still succeeded, by what each level nests.
///
/// | each level      | debug 2 MB | debug 16 MB | release 2 MB | release 16 MB |
/// |-----------------|-----------:|------------:|-------------:|--------------:|
/// | `(`             |        142 |       1,168 |          470 |         3,758 |
/// | `if a {`        |         76 |         623 |          293 |         2,341 |
/// | `g(\|\| {` (2)  |         63 |         515 |          225 |         1,799 |
/// | `\|\| {`        |         43 |         343 |          161 |         1,284 |
///
/// A quarter of the worst 2 MB debug figure (43) is about 10, below the floor
/// of 64, and a 2 MB thread cannot even hold 64 levels of closures. So the cap
/// is the floor, 64, and the parse runs on its own 16 MB thread
/// (`PARSE_STACK_BYTES` in `lib.rs`), where a quarter of the worst debug
/// figure is 85: 64 is within it on any build, whatever thread calls.
pub const MAX_NESTING: usize = 64;

/// The 1-based line of the first delimiter, in source order, opened deeper
/// than [`MAX_NESTING`]; `None` if there is none.
///
/// Mirrors `syn::parse_file`'s prologue: a BOM is ignored, and a `#!` first
/// line may or may not be a shebang that `syn` drops, so both readings are
/// checked. A text that does not lex is not too deep: `syn` lexes it the same
/// way and fails before parsing anything.
pub fn too_deep_at(src: &str) -> Option<u32> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let without_first_line = src
        .starts_with("#!")
        .then(|| src.find('\n').map_or("", |newline| &src[newline..]));
    first_too_deep(src).or_else(|| without_first_line.and_then(first_too_deep))
}

/// [`too_deep_at`] for one reading of the text.
fn first_too_deep(text: &str) -> Option<u32> {
    let tokens: TokenStream = text.parse().ok()?;
    // One iterator per level; the stack's length is the depth a group found
    // by the top iterator opens.
    let mut stack = vec![tokens.into_iter()];
    while let Some(level) = stack.last_mut() {
        match level.next() {
            Some(TokenTree::Group(group)) => {
                if stack.len() > MAX_NESTING {
                    return Some(group.span_open().start().line as u32);
                }
                stack.push(group.stream().into_iter());
            }
            Some(_) => {}
            None => {
                stack.pop();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parens(depth: usize) -> String {
        format!("{}{}", "(".repeat(depth), ")".repeat(depth))
    }

    #[test]
    fn depth_at_the_cap_passes_and_one_more_is_reported() {
        assert_eq!(too_deep_at(&parens(MAX_NESTING)), None);
        assert_eq!(
            too_deep_at(&format!("\n{}", parens(MAX_NESTING + 1))),
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
        assert_eq!(too_deep_at(&src), Some(2));
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
        assert_eq!(too_deep_at(&format!("{open}{close}")), Some(1));
        assert_eq!(too_deep_at("([{}])"), None);
    }

    #[test]
    fn a_text_that_does_not_lex_is_left_to_syn() {
        assert_eq!(too_deep_at(&"(".repeat(MAX_NESTING * 2)), None);
    }

    #[test]
    fn a_bom_is_ignored() {
        let src = format!("\u{feff}{}", parens(MAX_NESTING + 1));
        assert_eq!(too_deep_at(&src), Some(1));
    }

    #[test]
    fn a_shebang_line_that_does_not_lex_does_not_hide_the_rest() {
        let src = format!("#!/bin/sh '\n{}", parens(MAX_NESTING + 1));
        assert_eq!(too_deep_at(&src), Some(2));
    }
}
