//! Selecting, summarizing and printing scored functions: the table and JSON outputs.
//!
//! JSON is built with `serde_json::json!` rather than `#[derive(Serialize)]`:
//! the derive pulls `syn` and `proc-macro2` into every binary, and the JS alias
//! must build without them.

use crate::language::Lang;
use crate::model::Func;
use colored::Colorize;
use serde_json::{json, Value};
use std::io::{self, Write};

/// One scored function and where it came from.
#[derive(Clone)]
pub struct Row {
    /// The path as given or walked, or `<stdin>`.
    pub file: String,
    pub lang: Lang,
    pub func: Func,
}

/// Totals over the rows being reported, except `over`, which the run counts
/// over every scored function so `--min`/`--top` cannot hide a gate failure.
#[derive(Clone)]
pub struct Summary {
    pub funcs: usize,
    pub files: usize,
    pub mean: f64,
    pub worst: Option<Row>,
    /// The `--max` in force, if any.
    pub max: Option<u32>,
    /// Functions scoring above `max` (see [`over_limit`]); 0 when `max` is unset.
    pub over: usize,
}

/// How many of `rows` score above `max`; 0 when `max` is unset.
pub fn over_limit(rows: &[Row], max: Option<u32>) -> usize {
    max.map_or(0, |m| rows.iter().filter(|r| r.func.complexity > m).count())
}

pub fn summarize(rows: &[Row], max: Option<u32>) -> Summary {
    let mut files: Vec<&str> = rows.iter().map(|r| r.file.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    let total: u32 = rows.iter().map(|r| r.func.complexity).sum();
    let mean = if rows.is_empty() {
        0.0
    } else {
        f64::from(total) / rows.len() as f64
    };
    let worst = rows.iter().max_by_key(|r| r.func.complexity).cloned();
    let over = over_limit(rows, max);
    Summary {
        funcs: rows.len(),
        files: files.len(),
        mean,
        worst,
        max,
        over,
    }
}

/// Which rows to report and in what order.
#[derive(Clone, Copy, Debug)]
pub struct Filter {
    /// `--min`: drop rows scoring below this.
    pub min: u32,
    /// `--top`: keep only the N worst.
    pub top: Option<usize>,
    /// `--sort`: highest score first.
    pub sort: bool,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            min: 1,
            top: None,
            sort: false,
        }
    }
}

/// Filter and order rows for display.
///
/// `--min` drops low scores, `--top` keeps the N worst, and `--sort` orders by
/// score descending. Without `--sort` the surviving rows are restored to source
/// order so the table still reads file by file.
pub fn select(rows: Vec<Row>, filter: &Filter) -> Vec<Row> {
    let mut kept: Vec<(usize, Row)> = rows
        .into_iter()
        .enumerate()
        .filter(|(_, r)| r.func.complexity >= filter.min)
        .collect();

    if filter.top.is_some() || filter.sort {
        kept.sort_by_key(|(_, r)| std::cmp::Reverse(r.func.complexity));
    }
    if let Some(n) = filter.top {
        kept.truncate(n);
    }
    if !filter.sort {
        kept.sort_by_key(|(i, _)| *i);
    }
    kept.into_iter().map(|(_, r)| r).collect()
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Which display options the table writer needs.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub header: bool,
    pub explain: bool,
    /// One flat table with a Location column, instead of a section per file.
    pub flat: bool,
}

/// Column layout: score right-aligned in 6, line right-aligned in 7, two
/// spaces, kind left-aligned in 13, then two spaces of indent per nesting level.
///
/// `--sort` orders rows by score across files, which makes grouping by file
/// meaningless — a file's header would reappear every time its rows resurface.
/// Sorted output goes out flat instead, every row carrying its own `file:line`.
pub fn write_table<W: Write>(
    w: &mut W,
    rows: &[Row],
    summary: &Summary,
    style: Style,
) -> io::Result<()> {
    if style.flat {
        write_flat(w, rows, style)?;
    } else {
        write_grouped(w, rows, style)?;
    }
    if !rows.is_empty() {
        writeln!(w)?;
    }
    write_summary(w, summary)
}

/// A section per file, rows in source order and indented by nesting depth.
fn write_grouped<W: Write>(w: &mut W, rows: &[Row], style: Style) -> io::Result<()> {
    let mut current = "";
    for row in rows {
        if row.file != current {
            if !current.is_empty() {
                writeln!(w)?;
            }
            writeln!(w, "{}", row.file.bold())?;
            if style.header {
                writeln!(w, "{:>6}{:>7}  {:<13}Name", "CC", "Line", "Kind")?;
            }
            current = &row.file;
        }
        let f = &row.func;
        writeln!(
            w,
            "{:>6}{:>7}  {:<13}{}{}",
            f.complexity,
            f.line,
            f.kind.label,
            " ".repeat(2 * f.depth as usize),
            f.name
        )?;
        // 6 for the score, 7 for the line: points sit under the Line column.
        write_points(w, f, style.explain, 13)?;
    }
    Ok(())
}

/// One table, no file sections. The Location column is widened to the longest
/// `file:line` so the Kind and Name columns still line up. Depth is not shown:
/// a sorted row rarely sits next to the function it nests in.
fn write_flat<W: Write>(w: &mut W, rows: &[Row], style: Style) -> io::Result<()> {
    let locs: Vec<String> = rows
        .iter()
        .map(|r| format!("{}:{}", r.file, r.func.line))
        .collect();
    let width = locs
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(0)
        .max("Location".len());

    // No rows, no header: an empty flat table is just the summary, the way an
    // empty grouped one is.
    if style.header && !rows.is_empty() {
        writeln!(w, "{:>6}  {:<width$}  {:<13}Name", "CC", "Location", "Kind")?;
    }
    for (row, loc) in rows.iter().zip(&locs) {
        let f = &row.func;
        // Pad by hand: `loc` is colored, and a width spec would count the
        // escape bytes and shift the columns right.
        let pad = " ".repeat(width - loc.len());
        writeln!(
            w,
            "{:>6}  {}{}  {:<13}{}",
            f.complexity,
            loc.bold(),
            pad,
            f.kind.label,
            f.name
        )?;
        // Right-aligned on the Location column's far edge, so a point's line
        // lands under the location it belongs to and its branch under Kind.
        write_points(w, f, style.explain, 8 + width)?;
    }
    Ok(())
}

/// The decision points behind one score, each right-aligned in `width` so it
/// falls under whichever column carries the function's own line number.
fn write_points<W: Write>(w: &mut W, f: &Func, explain: bool, width: usize) -> io::Result<()> {
    if !explain {
        return Ok(());
    }
    for p in &f.points {
        writeln!(w, "{:>width$}  {}", p.line, p.branch)?;
    }
    Ok(())
}

/// The one-line summary, then the over-limit line when `--max` was exceeded.
fn write_summary<W: Write>(w: &mut W, summary: &Summary) -> io::Result<()> {
    write!(
        w,
        "{} function{} in {} file{}",
        summary.funcs,
        plural(summary.funcs),
        summary.files,
        plural(summary.files)
    )?;
    if let Some(worst) = &summary.worst {
        write!(
            w,
            " · mean {:.1} · worst {} ({} at {}:{})",
            summary.mean, worst.func.complexity, worst.func.name, worst.file, worst.func.line
        )?;
    }
    writeln!(w)?;
    if let (Some(max), true) = (summary.max, summary.over > 0) {
        writeln!(
            w,
            "{} function{} above the limit of {}",
            summary.over,
            plural(summary.over),
            max
        )?;
    }
    Ok(())
}

fn json_function(row: &Row) -> Value {
    let f = &row.func;
    let points: Vec<Value> = f
        .points
        .iter()
        .map(|p| json!({ "line": p.line, "branch": p.branch }))
        .collect();
    json!({
        "file": row.file,
        "language": row.lang.id(),
        "name": f.name,
        "kind": f.kind.id,
        "line": f.line,
        "depth": f.depth,
        "complexity": f.complexity,
        "points": points,
    })
}

fn json_summary(summary: &Summary) -> Value {
    let worst = summary.worst.as_ref().map(|r| {
        json!({
            "file": r.file,
            "name": r.func.name,
            "line": r.func.line,
            "complexity": r.func.complexity,
        })
    });
    json!({
        "functions": summary.funcs,
        "files": summary.files,
        "mean": summary.mean,
        "limit": summary.max,
        "overLimit": summary.over,
        "worst": worst,
    })
}

/// The whole report as one line of JSON.
pub fn write_json<W: Write>(w: &mut W, rows: &[Row], summary: &Summary) -> io::Result<()> {
    let report = json!({
        "summary": json_summary(summary),
        "functions": rows.iter().map(json_function).collect::<Vec<_>>(),
    });
    serde_json::to_writer(&mut *w, &report).map_err(io::Error::other)?;
    writeln!(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::{Lang, LangOptions, Language, OptionSpec};
    use crate::model::{Func, Kind, ParseError, Point};
    use std::path::Path;

    struct FakeLang(&'static str);
    impl Language for FakeLang {
        fn id(&self) -> &'static str {
            self.0
        }
        fn display_name(&self) -> &'static str {
            "Fake"
        }
        fn alias(&self) -> Option<&'static str> {
            None
        }
        fn extensions(&self) -> &'static [&'static str] {
            &[]
        }
        fn stdin_ext(&self) -> &'static str {
            ""
        }
        fn options(&self) -> &'static [OptionSpec] {
            &[]
        }
        fn skip(&self, _: &Path, _: &LangOptions) -> bool {
            false
        }
        fn analyze(&self, _: &str, _: &Path, _: &LangOptions) -> Result<Vec<Func>, ParseError> {
            Ok(vec![])
        }
    }
    static RUST: FakeLang = FakeLang("rust");
    static JS: FakeLang = FakeLang("js");

    const FN: Kind = Kind {
        id: "fn",
        label: "fn",
    };
    const METHOD: Kind = Kind {
        id: "method",
        label: "method",
    };
    const CLOSURE: Kind = Kind {
        id: "closure",
        label: "closure",
    };
    const TRAIT_FN: Kind = Kind {
        id: "trait_fn",
        label: "trait fn",
    };

    fn func(name: &str, kind: Kind, line: u32, cc: u32, depth: u32) -> Func {
        Func {
            name: name.to_string(),
            kind,
            line,
            complexity: cc,
            depth,
            points: Vec::new(),
        }
    }

    fn row_in(lang: Lang, file: &str, func: Func) -> Row {
        Row {
            file: file.to_string(),
            lang,
            func,
        }
    }

    fn row(file: &str, func: Func) -> Row {
        row_in(&RUST, file, func)
    }

    fn sample() -> Vec<Row> {
        vec![
            row("src/a.rs", func("validate", FN, 12, 14, 0)),
            row("src/a.rs", func("map", CLOSURE, 61, 3, 1)),
            row("src/b.rs", func("S::go", METHOD, 4, 2, 0)),
        ]
    }

    fn table(rows: &[Row], max: Option<u32>, style: Style) -> String {
        colored::control::set_override(false);
        let summary = summarize(rows, max);
        let mut out = Vec::new();
        write_table(&mut out, rows, &summary, style).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn render(rows: &[Row], header: bool, explain: bool) -> String {
        table(
            rows,
            None,
            Style {
                header,
                explain,
                flat: false,
            },
        )
    }

    fn render_flat(rows: &[Row], header: bool, explain: bool) -> String {
        table(
            rows,
            None,
            Style {
                header,
                explain,
                flat: true,
            },
        )
    }

    fn json(rows: &[Row], max: Option<u32>) -> serde_json::Value {
        let summary = summarize(rows, max);
        let mut out = Vec::new();
        write_json(&mut out, rows, &summary).unwrap();
        serde_json::from_slice(&out).unwrap()
    }

    #[test]
    fn table_groups_by_file_and_indents_by_depth() {
        let expected = "\
src/a.rs
    CC   Line  Kind         Name
    14     12  fn           validate
     3     61  closure        map

src/b.rs
    CC   Line  Kind         Name
     2      4  method       S::go

3 functions in 2 files · mean 6.3 · worst 14 (validate at src/a.rs:12)
";
        assert_eq!(render(&sample(), true, false), expected);
    }

    #[test]
    fn the_kind_column_shows_the_label_not_the_id() {
        let rows = vec![row("a.rs", func("T::f", TRAIT_FN, 1, 1, 0))];
        assert!(render(&rows, true, false).contains("     1      1  trait fn     T::f\n"));
    }

    #[test]
    fn no_header_drops_the_column_row() {
        let out = render(&sample(), false, false);
        assert!(!out.contains("CC   Line"));
        assert!(out.contains("    14     12  fn           validate"));
    }

    #[test]
    fn explain_lists_the_decision_points() {
        let mut rows = sample();
        rows[0].func.points = vec![
            Point {
                line: 13,
                branch: "if",
            },
            Point {
                line: 14,
                branch: "&&",
            },
        ];
        let out = render(&rows, true, true);
        assert!(out.contains("           13  if\n"), "got:\n{out}");
        assert!(out.contains("           14  &&\n"), "got:\n{out}");
    }

    #[test]
    fn summary_is_singular_for_one_function() {
        let rows = vec![row("a.rs", func("f", FN, 1, 2, 0))];
        let out = render(&rows, true, false);
        assert!(
            out.contains("1 function in 1 file · mean 2.0 · worst 2 (f at a.rs:1)"),
            "got:\n{out}"
        );
    }

    #[test]
    fn empty_input_reports_nothing_found() {
        assert_eq!(render(&[], true, false), "0 functions in 0 files\n");
    }

    /// Rows sorted by score interleave the files they came from. Grouping by
    /// file would reprint a header every time a file resurfaced, so sorted
    /// output is one flat table keyed by location.
    fn interleaved() -> Vec<Row> {
        vec![
            row("src/a.rs", func("validate", FN, 12, 14, 0)),
            row("src/b.rs", func("S::go", METHOD, 4, 9, 0)),
            row("src/a.rs", func("map", CLOSURE, 61, 3, 1)),
        ]
    }

    #[test]
    fn sorted_output_is_one_flat_table() {
        let out = render_flat(&interleaved(), true, false);
        assert_eq!(out.matches("Location").count(), 1, "got:\n{out}");
        assert!(!out.contains("\nsrc/a.rs\n"), "got:\n{out}");
        assert!(!out.contains("\nsrc/b.rs\n"), "got:\n{out}");
    }

    #[test]
    fn flat_columns_align_on_the_longest_location() {
        let out = render_flat(&interleaved(), true, false);
        let kind_col = |line: &str| {
            line.find("fn ")
                .or(line.find("method"))
                .or(line.find("closure"))
        };
        let cols: Vec<Option<usize>> = out.lines().skip(1).take(3).map(kind_col).collect();
        assert!(
            cols.windows(2).all(|w| w[0] == w[1]),
            "kind column ragged: {cols:?}\n{out}"
        );
    }

    #[test]
    fn flat_layout_drops_the_depth_indent() {
        let out = render_flat(&interleaved(), true, false);
        assert!(out.contains("closure      map\n"), "got:\n{out}");
    }

    #[test]
    fn no_header_drops_the_flat_column_row() {
        let out = render_flat(&interleaved(), false, false);
        assert!(!out.contains("Location"), "got:\n{out}");
    }

    #[test]
    fn empty_flat_table_is_just_the_summary() {
        assert_eq!(render_flat(&[], true, false), "0 functions in 0 files\n");
    }

    #[test]
    fn flat_explain_lines_sit_against_the_location_column() {
        let mut rows = interleaved();
        rows[0].func.points = vec![Point {
            line: 13,
            branch: "if",
        }];
        let out = render_flat(&rows, true, true);
        // Location column is 11 wide ("src/a.rs:12"), so points right-align at 19.
        assert!(out.contains("\n                 13  if\n"), "got:\n{out}");
    }

    #[test]
    fn over_limit_line_appears_only_when_max_is_exceeded() {
        let style = Style {
            header: true,
            explain: false,
            flat: false,
        };
        let over = table(&sample(), Some(2), style);
        assert!(
            over.ends_with(
                "· worst 14 (validate at src/a.rs:12)\n2 functions above the limit of 2\n"
            ),
            "got:\n{over}"
        );
        let one = table(&sample(), Some(3), style);
        assert!(
            one.ends_with("\n1 function above the limit of 3\n"),
            "got:\n{one}"
        );
        let within = table(&sample(), Some(14), style);
        assert!(!within.contains("above the limit"), "got:\n{within}");
        let unset = table(&sample(), None, style);
        assert!(!unset.contains("above the limit"), "got:\n{unset}");
    }

    fn selected(filter: Filter, rows: Vec<Row>) -> Vec<String> {
        select(rows, &filter)
            .into_iter()
            .map(|r| r.func.name)
            .collect()
    }

    fn spread() -> Vec<Row> {
        vec![
            row("a.rs", func("low", FN, 1, 1, 0)),
            row("a.rs", func("high", FN, 2, 9, 0)),
            row("b.rs", func("mid", FN, 3, 5, 0)),
        ]
    }

    #[test]
    fn min_filters_below_the_threshold() {
        let filter = Filter {
            min: 5,
            ..Filter::default()
        };
        assert_eq!(selected(filter, spread()), ["high", "mid"]);
    }

    #[test]
    fn min_of_one_keeps_everything_in_source_order() {
        assert_eq!(
            selected(Filter::default(), spread()),
            ["low", "high", "mid"]
        );
    }

    #[test]
    fn sort_orders_by_score_descending() {
        let filter = Filter {
            sort: true,
            ..Filter::default()
        };
        assert_eq!(selected(filter, spread()), ["high", "mid", "low"]);
    }

    #[test]
    fn top_keeps_the_n_worst_and_restores_source_order() {
        let filter = Filter {
            top: Some(2),
            ..Filter::default()
        };
        assert_eq!(selected(filter, spread()), ["high", "mid"]);
    }

    #[test]
    fn top_with_sort_stays_sorted() {
        let filter = Filter {
            top: Some(2),
            sort: true,
            ..Filter::default()
        };
        assert_eq!(selected(filter, spread()), ["high", "mid"]);
    }

    #[test]
    fn json_carries_every_row_and_the_summary() {
        let mut rows = sample();
        rows[0].func.points = vec![Point {
            line: 13,
            branch: "if",
        }];
        let v = json(&rows, None);

        assert_eq!(v["functions"].as_array().unwrap().len(), 3);
        assert_eq!(v["functions"][0]["file"], "src/a.rs");
        assert_eq!(v["functions"][0]["name"], "validate");
        assert_eq!(v["functions"][0]["kind"], "fn");
        assert_eq!(v["functions"][0]["complexity"], 14);
        assert_eq!(v["functions"][0]["line"], 12);
        assert_eq!(v["functions"][0]["depth"], 0);
        assert_eq!(v["functions"][0]["points"][0]["line"], 13);
        assert_eq!(v["functions"][0]["points"][0]["branch"], "if");
        assert_eq!(v["functions"][1]["kind"], "closure");

        assert_eq!(v["summary"]["functions"], 3);
        assert_eq!(v["summary"]["files"], 2);
        assert_eq!(v["summary"]["worst"]["name"], "validate");
    }

    #[test]
    fn json_has_both_tools_summary_fields() {
        let v = json(&sample(), Some(2));
        let summary = v["summary"].as_object().unwrap();
        let mut keys: Vec<&str> = summary.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["files", "functions", "limit", "mean", "overLimit", "worst"]
        );
        assert_eq!(v["summary"]["limit"], 2);
        assert_eq!(v["summary"]["overLimit"], 2);
        assert_eq!(
            v["summary"]["worst"],
            serde_json::json!({"file": "src/a.rs", "name": "validate", "line": 12, "complexity": 14})
        );
    }

    #[test]
    fn json_limit_is_null_and_over_limit_zero_without_max() {
        let v = json(&sample(), None);
        assert!(v["summary"]["limit"].is_null());
        assert_eq!(v["summary"]["overLimit"], 0);
    }

    #[test]
    fn json_worst_is_null_when_there_are_no_functions() {
        let v = json(&[], Some(3));
        assert!(v["summary"].as_object().unwrap().contains_key("worst"));
        assert!(v["summary"]["worst"].is_null());
        assert_eq!(v["functions"], serde_json::json!([]));
    }

    #[test]
    fn json_functions_carry_their_language() {
        let rows = vec![
            row_in(&JS, "a.ts", func("f", FN, 1, 1, 0)),
            row_in(&RUST, "b.rs", func("g", TRAIT_FN, 2, 1, 0)),
        ];
        let v = json(&rows, None);
        assert_eq!(v["functions"][0]["language"], "js");
        assert_eq!(v["functions"][1]["language"], "rust");
        assert_eq!(
            v["functions"][1]["kind"], "trait_fn",
            "kind is the id, not the label"
        );
    }

    #[test]
    fn json_is_a_single_line() {
        let summary = summarize(&sample(), None);
        let mut out = Vec::new();
        write_json(&mut out, &sample(), &summary).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 1, "got:\n{text}");
        assert!(text.ends_with('\n'));
    }
}
