use cmplx_rust::analyze::{analyze, Options};
use std::collections::BTreeMap;

fn score_file(path: &str, opts: Options) -> BTreeMap<String, u32> {
    let src = std::fs::read_to_string(path).expect("fixture should exist");
    analyze(&src, opts)
        .expect("fixture should parse")
        .into_iter()
        .map(|f| (f.name, f.complexity))
        .collect()
}

fn expect(path: &str, pairs: &[(&str, u32)]) {
    let got = score_file(path, Options::default());
    for (name, want) in pairs {
        assert_eq!(
            got.get(*name),
            Some(want),
            "{path}: {name} should score {want}; full result: {got:?}"
        );
    }
}

#[test]
fn nesting_fixture() {
    expect(
        "fixtures/nesting.rs",
        &[
            ("Engine::allows", 3),
            ("Engine::tally", 2),
            ("filter", 2),
            ("Policy::check", 2),
            ("outer", 2),
            ("inner", 2),
        ],
    );
}

#[test]
fn patterns_fixture() {
    expect(
        "fixtures/patterns.rs",
        &[("describe", 8), ("unwrap_pair", 4), ("parse", 1)],
    );
}

#[test]
fn question_flag_changes_only_the_question_operator() {
    let with = score_file("fixtures/patterns.rs", Options { question: true, ..Options::default() });
    assert_eq!(with.get("parse"), Some(&3), "two ? operators");
    assert_eq!(with.get("describe"), Some(&8), "unchanged by --question");
}

#[test]
fn syntax_fixture_is_not_confused_by_lexical_traps() {
    expect(
        "fixtures/syntax.rs",
        &[("Holder::shout", 2), ("ambiguous", 3), ("r#match", 1), ("uses_macro", 1)],
    );
}

#[test]
fn nested_functions_report_their_depth() {
    let src = std::fs::read_to_string("fixtures/nesting.rs").unwrap();
    let funcs = analyze(&src, Options::default()).unwrap();
    let inner = funcs.iter().find(|f| f.name == "inner").unwrap();
    assert_eq!(inner.depth, 1);
}
