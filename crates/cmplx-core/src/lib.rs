//! Language-agnostic core for the cmplx cyclomatic-complexity tools.

pub mod cli;
pub mod config;
pub mod language;
pub mod lsp;
pub mod model;
pub mod report;
pub mod run;
pub mod supervisor;
pub mod walk;
pub mod worker;

pub use language::MAX_NESTING;
pub use report::MAX_SHOWN_COMPLEXITY;
