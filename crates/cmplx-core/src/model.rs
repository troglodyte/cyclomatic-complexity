//! Language-agnostic result types shared by every language crate.

/// The kind of a function-like item. `id` is the stable JSON value;
/// `label` is the human-facing text for tables and the LSP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kind {
    pub id: &'static str,
    pub label: &'static str,
}

/// A single branch point that contributed to a function's complexity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub line: u32,
    pub branch: &'static str,
}

/// The analysis result for one function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Func {
    pub name: String,
    pub kind: Kind,
    pub line: u32,
    pub complexity: u32,
    pub depth: u32,
    pub points: Vec<Point>,
}

/// A source file that could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: u32,
    pub message: String,
    pub kind: ParseErrorKind,
}

/// Why a source was not scored, which decides how the CLI and LSP show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// Not valid source, as the language reported it. Usually passing, while
    /// someone is typing.
    Syntax,
    /// Brackets nested deeper than `limit` levels (the `ParseError`'s line is
    /// where the depth was first exceeded), so core never handed it to the
    /// language. Lasts until the file is edited.
    TooDeep { limit: usize },
}
