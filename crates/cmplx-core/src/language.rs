//! The `Language` trait, per-language option plumbing, and the language registry.

use crate::model::{Func, ParseError, ParseErrorKind};
use std::collections::BTreeMap;
use std::path::Path;

/// Describes one boolean option a language exposes (config key, CLI flags, LSP key).
#[derive(Clone, Copy, Debug)]
pub struct OptionSpec {
    /// `cmplx.toml` key and `initializationOptions` key, e.g. `"optional-chain"`.
    pub key: &'static str,
    /// Flag that turns the option on, e.g. `Some("--optional-chain")`.
    pub on_flag: Option<&'static str>,
    /// Flag that turns the option off, e.g. `Some("--no-logical")`.
    pub off_flag: Option<&'static str>,
    /// `None` means the language decides per file (e.g. jsx).
    pub default: Option<bool>,
    /// `--help` text for the flags listed, so it says what *they* do:
    /// `--no-logical` reads "do not count …", not "count …".
    pub help: &'static str,
}

/// Explicitly set option values for one language.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LangOptions(BTreeMap<&'static str, bool>);

impl LangOptions {
    /// The explicitly set value only.
    pub fn get(&self, key: &str) -> Option<bool> {
        self.0.get(key).copied()
    }

    /// The set value, else the spec's default.
    pub fn flag(&self, spec: &OptionSpec) -> Option<bool> {
        self.get(spec.key).or(spec.default)
    }

    pub fn set(&mut self, key: &'static str, value: bool) {
        self.0.insert(key, value);
    }

    /// Every explicitly set value, in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, bool)> + '_ {
        self.0.iter().map(|(key, value)| (*key, *value))
    }

    /// Layer `self` over `lower`; `self` wins on conflicts.
    pub fn or(self, lower: LangOptions) -> LangOptions {
        let mut merged = lower.0;
        merged.extend(self.0);
        LangOptions(merged)
    }
}

pub trait Language: Sync {
    /// `"js"`: also the `--js` flag and the `[js]` table.
    fn id(&self) -> &'static str;
    /// `"JavaScript/TypeScript"`, for `--help`.
    fn display_name(&self) -> &'static str;
    fn alias(&self) -> Option<&'static str>;
    /// Lowercase, without the dot; files match them ignoring case.
    fn extensions(&self) -> &'static [&'static str];
    /// Used when `-` is read and `--ext` is absent.
    fn stdin_ext(&self) -> &'static str;
    fn options(&self) -> &'static [OptionSpec];
    fn skip(&self, path: &Path, opts: &LangOptions) -> bool;
    /// The 1-based line on which `(`, `[` and `{` first nest deeper than
    /// `limit` levels, or `None` if they never do. Brackets inside strings,
    /// chars, comments and other literals do not count. One iterative pass
    /// that stops at that line, so any depth is safe to scan.
    fn first_line_deeper_than(
        &self,
        src: &str,
        path: &Path,
        opts: &LangOptions,
        limit: usize,
    ) -> Option<u32>;
    /// Score `src`. Core calls this only after [`Self::first_line_deeper_than`]
    /// found nothing deeper than [`MAX_NESTING`] (see [`analyze_bounded`]), so
    /// it may recurse on bracket nesting up to that depth.
    fn analyze(&self, src: &str, path: &Path, opts: &LangOptions) -> Result<Vec<Func>, ParseError>;
}

/// Deepest bracket nesting any language scores. Deeper code is refused before
/// it is analyzed: it cannot crash a recursive parser, and it is far past
/// anything a person should maintain.
pub const MAX_NESTING: usize = 100;

/// Refuse `src` if its brackets nest deeper than [`MAX_NESTING`], else have
/// `lang` analyze it. Every caller in core scores through this.
pub fn analyze_bounded(
    lang: &dyn Language,
    src: &str,
    path: &Path,
    opts: &LangOptions,
) -> Result<Vec<Func>, ParseError> {
    match lang.first_line_deeper_than(src, path, opts, MAX_NESTING) {
        Some(line) => Err(ParseError {
            line,
            message: format!(
                "nesting deeper than {MAX_NESTING} levels; this code needs restructuring"
            ),
            kind: ParseErrorKind::TooDeep { limit: MAX_NESTING },
        }),
        None => lang.analyze(src, path, opts),
    }
}

pub type Lang = &'static dyn Language;

#[derive(Clone)]
pub struct Registry {
    langs: Vec<Lang>,
    complete: bool,
}

impl Registry {
    /// Every language, as in the `cmplx` binary.
    pub fn all(langs: &[Lang]) -> Registry {
        Registry {
            langs: langs.to_vec(),
            complete: true,
        }
    }

    /// A single-language registry, as in the alias binaries.
    pub fn alias(lang: Lang) -> Registry {
        Registry {
            langs: vec![lang],
            complete: false,
        }
    }

    pub fn langs(&self) -> &[Lang] {
        &self.langs
    }

    pub fn by_id(&self, id: &str) -> Option<Lang> {
        self.langs.iter().copied().find(|lang| lang.id() == id)
    }

    /// The language within `active` whose extensions match `path`, ignoring
    /// ASCII case (`Y.JS` is JavaScript); languages list their extensions lowercase.
    pub fn for_path(&self, path: &Path, active: &[Lang]) -> Option<Lang> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        active
            .iter()
            .copied()
            .find(|lang| lang.extensions().contains(&ext.as_str()))
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
            &[]
        }
        fn skip(&self, _: &Path, _: &LangOptions) -> bool {
            false
        }
        fn first_line_deeper_than(&self, _: &str, _: &Path, _: &LangOptions, _: usize) -> Option<u32> {
            None
        }
        fn analyze(&self, _: &str, _: &Path, _: &LangOptions) -> Result<Vec<Func>, ParseError> {
            Ok(vec![])
        }
    }
    static FAKE: Fake = Fake;

    fn spec_with_default(default: Option<bool>) -> OptionSpec {
        OptionSpec {
            key: "logical",
            on_flag: None,
            off_flag: None,
            default,
            help: "",
        }
    }

    fn opts(pairs: &[(&'static str, bool)]) -> LangOptions {
        let mut o = LangOptions::default();
        for (k, v) in pairs {
            o.set(k, *v);
        }
        o
    }

    /// Nests by `(` and `)` anywhere in the text; `analyze` scores one
    /// function per call and panics on a line of 101 or more `(`, so a test
    /// fails loudly if the guard ever lets such a text through.
    struct Parens;
    impl Language for Parens {
        fn id(&self) -> &'static str {
            "parens"
        }
        fn display_name(&self) -> &'static str {
            "Parens"
        }
        fn alias(&self) -> Option<&'static str> {
            None
        }
        fn extensions(&self) -> &'static [&'static str] {
            &["pa"]
        }
        fn stdin_ext(&self) -> &'static str {
            "pa"
        }
        fn options(&self) -> &'static [OptionSpec] {
            &[]
        }
        fn skip(&self, _: &Path, _: &LangOptions) -> bool {
            false
        }
        fn first_line_deeper_than(&self, src: &str, _: &Path, _: &LangOptions, limit: usize) -> Option<u32> {
            let mut depth = 0usize;
            for (i, line) in src.lines().enumerate() {
                for c in line.chars() {
                    match c {
                        '(' => depth += 1,
                        ')' => depth = depth.saturating_sub(1),
                        _ => {}
                    }
                    if depth > limit {
                        return Some(i as u32 + 1);
                    }
                }
            }
            None
        }
        fn analyze(&self, src: &str, _: &Path, _: &LangOptions) -> Result<Vec<Func>, ParseError> {
            assert!(!src.contains(&"(".repeat(MAX_NESTING + 1)), "analyze saw a too-deep text");
            Ok(vec![Func {
                name: "f".into(),
                kind: crate::model::Kind { id: "fn", label: "fn" },
                line: 1,
                complexity: 1,
                depth: 0,
                points: vec![],
            }])
        }
    }

    fn bounded(src: &str) -> Result<Vec<Func>, ParseError> {
        analyze_bounded(&Parens, src, Path::new("a.pa"), &LangOptions::default())
    }

    #[test]
    fn the_nesting_cap_is_one_hundred_levels() {
        assert_eq!(MAX_NESTING, 100);
    }

    #[test]
    fn nesting_at_the_cap_is_analyzed() {
        let src = format!("{}{}", "(".repeat(MAX_NESTING), ")".repeat(MAX_NESTING));
        assert_eq!(bounded(&src).map(|funcs| funcs.len()), Ok(1));
    }

    #[test]
    fn nesting_past_the_cap_is_refused_by_core_at_the_line_it_is_first_exceeded() {
        let src = format!("x\n{}\n{}", "(".repeat(60), "(".repeat(41));
        assert_eq!(
            bounded(&src),
            Err(ParseError {
                line: 3,
                message: "nesting deeper than 100 levels; this code needs restructuring".into(),
                kind: ParseErrorKind::TooDeep { limit: 100 },
            })
        );
    }

    #[test]
    fn flag_falls_back_to_the_spec_default() {
        assert_eq!(
            LangOptions::default().flag(&spec_with_default(Some(true))),
            Some(true)
        );
        assert_eq!(LangOptions::default().flag(&spec_with_default(None)), None);
    }

    #[test]
    fn flag_prefers_an_explicit_value() {
        let o = opts(&[("logical", false)]);
        assert_eq!(o.flag(&spec_with_default(Some(true))), Some(false));
    }

    #[test]
    fn or_prefers_the_upper_layer() {
        let merged = opts(&[("logical", false)]).or(opts(&[("logical", true), ("tests", false)]));
        assert_eq!(merged.get("logical"), Some(false));
        assert_eq!(merged.get("tests"), Some(false));
    }

    #[test]
    fn for_path_picks_by_extension_within_active() {
        let registry = Registry::all(&[&FAKE]);
        assert!(registry.for_path(Path::new("a.fk"), &[&FAKE]).is_some());
        assert!(registry.for_path(Path::new("a.fk"), &[]).is_none());
    }

    #[test]
    fn for_path_ignores_the_case_of_the_extension() {
        let registry = Registry::all(&[&FAKE]);
        assert_eq!(
            registry.for_path(Path::new("a.FK"), &[&FAKE]).map(|l| l.id()),
            Some("fake")
        );
        assert_eq!(
            registry.for_path(Path::new("a.Fk"), &[&FAKE]).map(|l| l.id()),
            Some("fake")
        );
    }

    #[test]
    fn registry_completeness_and_lookup() {
        assert!(Registry::all(&[&FAKE]).is_complete());
        let alias = Registry::alias(&FAKE);
        assert!(!alias.is_complete());
        assert_eq!(alias.by_id("fake").map(|l| l.id()), Some("fake"));
        assert!(alias.by_id("other").is_none());
    }
}
