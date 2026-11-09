//! Bounds on nesting, checked before `syn` parses.
//!
//! `syn` is a recursive-descent parser and the analyzer's walk recurses with
//! it, so stack use grows with nesting depth. A stack overflow aborts the
//! process (`catch_unwind` cannot catch it), which would end a CLI run and the
//! shared language server. So a file is refused before `syn` sees it if any
//! of the constructs `syn` recurses on goes deeper than its cap, or several
//! together exceed a shared budget ([`COMBINED_BUDGET`]).
//!
//! Every cap is a quarter, rounded down, of the depth at which `parse_file` +
//! the analyzer's walk + drop overflowed in a debug build on the 16 MB parse
//! thread (syn 3.0.9, measured one shape per process through
//! `Language::analyze`). Constructs that `syn` keeps in a `Vec` (paths,
//! attributes, statements, items, array elements, match arms, or-patterns,
//! where-clauses, bounds) survived 1,000,000 repeats and are not capped.
//! The counters are heuristics on tokens, not a parse; they lean towards
//! over-counting, which can only leave an odd file unscored, never misscore.
//!
//! The depth is counted on proc-macro2's token tree. Its lexer keeps an explicit
//! stack (`parse::token_stream` in proc-macro2 1.0.107) and its `TokenStream`
//! drop is non-recursive, so lexing a deep file is safe; the tree is then
//! walked with an explicit stack here. Brackets inside strings, chars and
//! comments are not tokens, so they do not count.

use proc_macro2::{token_stream, Delimiter, Punct, Spacing, TokenStream, TokenTree};
use std::iter::Peekable;

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

/// Longest run of prefix operators: `&` (so `&&` is two), `*`, `!`, `-` and
/// `box`, with `mut`, `const`, `raw` and lifetimes continuing a run. Runs carry
/// into a group they precede, as `&(&(x))` nests. Overflow: 450 levels of `&`
/// types (451 `*const`, 450 `&'a`); expressions 3,956 (`!`, `-`, `*`) and
/// 5,802 (`&`); patterns 7,575 (`&`, `box`).
pub const MAX_PREFIX_OPERATORS: usize = 100;

/// Deepest generic nesting. A `<` opens after an identifier, `::`, `:`, `>`,
/// `<`, `=`, `,`, `&` or `*` or at the start of a group; a `>` closes, so `>>`
/// closes two; `->`, `=>` and `>=` do not. Comparisons therefore count as
/// opens until `;`, `=>` or the end of a block. Overflow: 286 (`Vec<`),
/// 287 (turbofish), 405 (`<T as A>::B`).
pub const MAX_GENERIC_DEPTH: usize = 70;

/// Longest chain of `->` in one type, as in `fn() -> fn() -> T`. Overflow:
/// 362 (`impl Fn() ->`), 394 (`fn() ->`).
pub const MAX_FN_ARROWS: usize = 90;

/// Most expression heads that each open a sub-expression running on to the
/// end of the statement or to a block: closure bars, `..`, `=` (and `+=` and
/// so on), `return`, `break`, `yield`, `if` (not after `else`), `while`,
/// `match`, `for`, `let` and the `@` of a pattern binding. A `|` or `.` counts
/// only where no operand precedes it, so `a || b` and `a.b` are not heads.
/// Overflow: 966 (`|a|` or `||` closures), 1,138 (`if` conditions), 1,167
/// (`for`), 1,299 (`while`), 1,357 (`match`), 1,387 (`break`), 1,414 (`..`),
/// 1,563 (`return`), 1,963 (`a =`), 5,502 (`a @`); `if let a =` (620) and
/// `while let a =` (664) count three heads each.
pub const MAX_EXPRESSION_HEADS: usize = 200;

/// Most operators in one expression: every punctuation token, `as`, and each
/// call or index (a `(` or `[` group after an operand). Overflow: 24,397
/// (`+`, `&&`), 26,227 (calls), 26,900 method calls (two each), 28,354 (`?`),
/// 29,974 (fields, indexes, `.await`, `as`).
pub const MAX_CHAIN_OPERATORS: usize = 6_000;

/// Longest `else if` chain. Overflow: 26,226 (`else if` and `else if let`).
pub const MAX_ELSE_IF_CHAIN: usize = 6_000;

/// Each counter at its cap uses about a quarter of the stack it overflowed
/// at, so the counters' shares of their caps may add up to at most this. Two
/// lets one construct reach its cap inside ordinary nesting while a mix of
/// several stays within about half the measured stack.
pub const COMBINED_BUDGET: usize = 2;

/// Why a file is refused, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TooDeep {
    /// 1-based line of the token that went over.
    pub line: u32,
    pub message: String,
}

/// The first point, in source order, where the text goes over a cap or the
/// combined budget; `None` if it never does.
///
/// Mirrors `syn::parse_file`'s prologue: a BOM is ignored, and a `#!` first
/// line may or may not be a shebang that `syn` drops, so both readings are
/// checked. A text that does not lex is not too deep: `syn` lexes it the same
/// way and fails before parsing anything.
pub fn too_deep_at(src: &str) -> Option<TooDeep> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let without_first_line = src
        .starts_with("#!")
        .then(|| src.find('\n').map_or("", |newline| &src[newline..]));
    first_too_deep(src).or_else(|| without_first_line.and_then(first_too_deep))
}

/// The counters at one point of the walk. A group inherits its opener's
/// counts as its floor, so nesting through brackets adds up.
#[derive(Clone, Copy, Default)]
struct Counts {
    prefix: usize,
    generics: usize,
    /// The highest `prefix` and `generics` since the expression began: in
    /// `&&&x + a + …` the walk of the `+` chain still holds the `&` frames, so
    /// the budget must see both together.
    prefix_peak: usize,
    generics_peak: usize,
    arrows: usize,
    heads: usize,
    operators: usize,
    else_ifs: usize,
}

/// `(count, cap, what is too deep)` for every counter.
fn limits(counts: &Counts) -> [(usize, usize, &'static str); 6] {
    [
        (counts.prefix_peak, MAX_PREFIX_OPERATORS, "prefix operators in a row"),
        (counts.generics_peak, MAX_GENERIC_DEPTH, "levels of generics"),
        (counts.arrows, MAX_FN_ARROWS, "levels of `->` types"),
        (
            counts.heads,
            MAX_EXPRESSION_HEADS,
            "nested closures, ranges, assignments, returns or breaks",
        ),
        (counts.operators, MAX_CHAIN_OPERATORS, "operators in one expression"),
        (counts.else_ifs, MAX_ELSE_IF_CHAIN, "branches in one else-if chain"),
    ]
}

/// The refusal message, if `depth` delimiters deep with `counts` is too deep.
fn over(depth: usize, counts: &Counts) -> Option<String> {
    let all = limits(counts);
    if depth > MAX_NESTING {
        return Some(format!("too deeply nested to score (> {MAX_NESTING} levels)"));
    }
    if let Some((_, cap, what)) = all.iter().find(|(count, cap, _)| count > cap) {
        return Some(format!("too deeply nested to score (> {cap} {what})"));
    }
    // Shares of each cap, in millionths so integer sums stay exact enough.
    const WHOLE: usize = 1_000_000;
    let share = |count: usize, cap: usize| count.saturating_mul(WHOLE) / cap;
    let used = all
        .iter()
        .fold(share(depth, MAX_NESTING), |sum, (count, cap, _)| sum + share(*count, *cap));
    (used > COMBINED_BUDGET * WHOLE)
        .then(|| "too deeply nested to score (several kinds of nesting together)".to_string())
}

/// What the previous token in a group was, as far as the counters care.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Prev {
    Start,
    /// An identifier, literal or group: something an operator can follow.
    Operand,
    /// A keyword after which an expression starts (`return`, `in`, ...).
    Keyword,
    Else,
    /// `fn`: the name or parameter list after it is a declaration, not a call.
    Fn,
    /// `mut`, `const`, `raw` or a lifetime: continues a prefix run.
    Continuer,
    Punct(char, Spacing),
}

/// One open group being walked.
struct Frame {
    tokens: Peekable<token_stream::IntoIter>,
    /// The counts the group was opened with; resets return to these.
    floor: Counts,
    now: Counts,
    prev: Prev,
    /// The previous token was a `{ }` group: a statement may have ended.
    after_brace: bool,
}

impl Frame {
    fn new(tokens: TokenStream, floor: Counts) -> Frame {
        Frame {
            tokens: tokens.into_iter().peekable(),
            floor,
            now: floor,
            prev: Prev::Start,
            after_brace: false,
        }
    }
}

/// [`too_deep_at`] for one reading of the text: one pass over the token tree
/// with an explicit stack of groups.
fn first_too_deep(text: &str) -> Option<TooDeep> {
    let tokens: TokenStream = text.parse().ok()?;
    let mut stack = vec![Frame::new(tokens, Counts::default())];
    while let Some(frame) = stack.last_mut() {
        let Some(token) = frame.tokens.next() else {
            stack.pop();
            continue;
        };
        let line = token.span().start().line as u32;
        let child = step(frame, &token);
        let depth = stack.len() - 1 + usize::from(child.is_some());
        let counts = child.as_ref().map_or(stack[stack.len() - 1].now, |c| c.now);
        if let Some(message) = over(depth, &counts) {
            return Some(TooDeep { line, message });
        }
        stack.extend(child);
    }
    None
}

/// Update `frame` for `token`; a group yields the frame to walk it with.
fn step(frame: &mut Frame, token: &TokenTree) -> Option<Frame> {
    // A block ends its statement unless the expression carries on past it
    // (`else` keeps an else-if chain going).
    if std::mem::take(&mut frame.after_brace) && !continues_after_block(token) {
        frame.now = frame.floor;
    }
    let (prev, child) = match token {
        TokenTree::Group(group) => {
            let call = frame.prev == Prev::Operand && group.delimiter() != Delimiter::Brace;
            frame.now.operators += usize::from(call);
            let child = Frame::new(group.stream(), frame.now);
            frame.now.prefix = frame.floor.prefix;
            frame.after_brace = group.delimiter() == Delimiter::Brace;
            (Prev::Operand, Some(child))
        }
        TokenTree::Ident(ident) => (ident_step(frame, &ident.to_string()), None),
        TokenTree::Literal(_) => {
            frame.now.prefix = frame.floor.prefix;
            (Prev::Operand, None)
        }
        TokenTree::Punct(punct) => {
            let next = match frame.tokens.peek() {
                Some(TokenTree::Punct(next)) => Some(next.as_char()),
                _ => None,
            };
            punct_step(frame, punct, next);
            (Prev::Punct(punct.as_char(), punct.spacing()), None)
        }
    };
    frame.prev = prev;
    let now = &mut frame.now;
    now.prefix_peak = now.prefix_peak.max(now.prefix);
    now.generics_peak = now.generics_peak.max(now.generics);
    child
}

/// Tokens that can carry an expression on past a `{ }` block, so the block
/// did not end the statement.
fn continues_after_block(token: &TokenTree) -> bool {
    match token {
        TokenTree::Punct(punct) => !matches!(punct.as_char(), '#' | ';' | ','),
        TokenTree::Ident(ident) => ident == "else" || ident == "as",
        _ => false,
    }
}

fn ident_step(frame: &mut Frame, name: &str) -> Prev {
    let now = &mut frame.now;
    if matches!(frame.prev, Prev::Punct('\'', _)) || matches!(name, "mut" | "const" | "raw") {
        return Prev::Continuer;
    }
    if frame.prev == Prev::Fn {
        return Prev::Fn;
    }
    if name == "box" {
        now.prefix += 1;
        return Prev::Keyword;
    }
    now.prefix = frame.floor.prefix;
    match name {
        "return" | "break" | "yield" | "while" | "match" | "for" | "let" => now.heads += 1,
        // `else if` is counted as a chain instead.
        "if" if frame.prev != Prev::Else => now.heads += 1,
        "as" => now.operators += 1,
        "if" if frame.prev == Prev::Else => now.else_ifs += 1,
        _ => {}
    }
    match name {
        "else" => Prev::Else,
        "fn" => Prev::Fn,
        "return" | "break" | "yield" | "in" | "if" | "match" | "while" | "let" | "move"
        | "static" | "async" | "for" | "loop" | "unsafe" | "as" | "dyn" | "impl" | "where" => {
            Prev::Keyword
        }
        _ => Prev::Operand,
    }
}

fn punct_step(frame: &mut Frame, punct: &Punct, next: Option<char>) {
    let (floor, prev) = (frame.floor, frame.prev);
    let now = &mut frame.now;
    let c = punct.as_char();
    let joint_next = |chars: &[char]| {
        punct.spacing() == Spacing::Joint && next.is_some_and(|n| chars.contains(&n))
    };
    let after_joint = |chars: &[char]| matches!(prev, Prev::Punct(p, Spacing::Joint) if chars.contains(&p));
    let after_operand = prev == Prev::Operand;
    if c != '\'' {
        now.operators += 1;
    }
    match c {
        '&' | '*' | '!' | '-' => now.prefix += 1,
        '\'' => {}
        _ => now.prefix = floor.prefix,
    }
    match c {
        ';' => *now = floor,
        ',' => {
            now.operators = floor.operators;
            now.heads = floor.heads;
            now.arrows = floor.arrows;
            now.prefix = floor.prefix;
            now.prefix_peak = floor.prefix_peak;
        }
        '=' if joint_next(&['>']) => {
            // `=>`: generics, heads and operators end with a match arm's pattern.
            now.generics = floor.generics;
            now.generics_peak = floor.generics_peak;
            now.heads = floor.heads;
            now.operators = floor.operators;
        }
        '=' if !after_joint(&['=', '!', '<', '>']) && !joint_next(&['=']) => now.heads += 1,
        '<' if opens_generics(prev) => now.generics += 1,
        '>' if after_joint(&['-']) => now.arrows += 1,
        '>' if after_joint(&['=']) || joint_next(&['=']) => {}
        '>' => now.generics = now.generics.saturating_sub(1).max(floor.generics),
        '|' | '.' if !after_operand => now.heads += 1,
        '@' => now.heads += 1,
        _ => {}
    }
}

/// Whether a `<` after `prev` may open generics (see [`MAX_GENERIC_DEPTH`]).
fn opens_generics(prev: Prev) -> bool {
    match prev {
        Prev::Start | Prev::Operand | Prev::Keyword | Prev::Continuer => true,
        Prev::Punct(c, _) => matches!(c, ':' | '>' | '<' | '=' | ',' | '&' | '*'),
        Prev::Else | Prev::Fn => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_of(src: &str) -> Option<u32> {
        too_deep_at(src).map(|too_deep| too_deep.line)
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
