//! The `Language` trait, per-language option plumbing, and the language registry.

use crate::model::{Func, ParseError};
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
    fn analyze(&self, src: &str, path: &Path, opts: &LangOptions) -> Result<Vec<Func>, ParseError>;
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
