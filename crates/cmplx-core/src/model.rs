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

/// Why a source could not be parsed, which decides how the LSP shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// Not valid source. Usually passing, while someone is typing.
    Syntax,
    /// Nested too deeply to parse safely. Lasts until the file is edited.
    TooDeep,
}
