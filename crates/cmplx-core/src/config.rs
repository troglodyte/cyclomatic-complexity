//! `cmplx.toml` (and the legacy `rustcx.toml`), shared by the CLI and the language server.
//!
//! Every value is optional so that "unset" is distinguishable from "set to the
//! value that happens to be the default"; that is what makes layering work.
//! Top-level boolean keys are expanded at parse time into every language that
//! declares them, then `[<id>]` tables override, so a merged config needs no
//! knowledge of where a value came from.

use crate::language::{Lang, LangOptions, Registry};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The name looked for when walking up from a starting directory.
pub const FILE_NAME: &str = "cmplx.toml";
/// Fallback file name and the id of the only language it configures.
pub const LEGACY_RUST_FILE: (&str, &str) = ("rustcx.toml", "rust");

/// Parsed configuration. Unset fields fall through to the next layer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileConfig {
    /// CLI: the default for `--max`. Unrelated to `threshold`.
    pub max: Option<u32>,
    /// LSP: warn at this score, error at twice it.
    pub threshold: Option<u32>,
    langs: BTreeMap<&'static str, LangOptions>,
}

/// A config file that could not be read or parsed.
#[derive(Clone, Debug)]
pub struct ConfigError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

/// The declared option key (`'static`) named `key` on `lang`, if any.
fn declared_key(lang: Lang, key: &str) -> Option<&'static str> {
    lang.options()
        .iter()
        .map(|spec| spec.key)
        .find(|k| *k == key)
}

fn declaring_langs<'a>(
    langs: &'a [Lang],
    key: &'a str,
) -> impl Iterator<Item = (Lang, &'static str)> + 'a {
    langs
        .iter()
        .copied()
        .filter_map(move |lang| declared_key(lang, key).map(|k| (lang, k)))
}

fn to_u32(value: &toml::Value) -> Option<u32> {
    value.as_integer().and_then(|n| u32::try_from(n).ok())
}

impl FileConfig {
    fn set(&mut self, lang: Lang, key: &'static str, value: bool) {
        self.langs.entry(lang.id()).or_default().set(key, value);
    }

    /// Parse `cmplx.toml` text. In a complete registry unknown keys and tables
    /// are errors; in an alias registry they are ignored when they could belong
    /// to a language that is not linked. A key inside a linked language's own
    /// table that it does not declare is always an error.
    pub fn parse(path: &Path, text: &str, reg: &Registry) -> Result<FileConfig, ConfigError> {
        let fail = |message: String| ConfigError {
            path: path.to_path_buf(),
            message,
        };
        let table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| fail(e.to_string()))?;
        let mut cfg = FileConfig::default();
        // Top-level keys first, so language tables override them whatever the file order.
        for (key, value) in &table {
            match key.as_str() {
                "max" => {
                    cfg.max = Some(
                        to_u32(value)
                            .ok_or_else(|| fail("`max` must be a non-negative integer".into()))?,
                    )
                }
                "threshold" => {
                    cfg.threshold =
                        Some(to_u32(value).ok_or_else(|| {
                            fail("`threshold` must be a non-negative integer".into())
                        })?)
                }
                _ if reg.by_id(key).is_some() && value.is_table() => {}
                _ => {
                    let owners: Vec<_> = declaring_langs(reg.langs(), key).collect();
                    if owners.is_empty() {
                        if reg.is_complete() {
                            return Err(fail(if value.is_table() {
                                format!("unknown table `[{key}]`")
                            } else {
                                format!("unknown key `{key}`")
                            }));
                        }
                        continue;
                    }
                    let flag = value
                        .as_bool()
                        .ok_or_else(|| fail(format!("`{key}` must be a boolean")))?;
                    for (lang, k) in owners {
                        cfg.set(lang, k, flag);
                    }
                }
            }
        }
        for (id, value) in &table {
            let (Some(lang), Some(options)) = (reg.by_id(id), value.as_table()) else {
                continue;
            };
            for (key, value) in options {
                let k = declared_key(lang, key)
                    .ok_or_else(|| fail(format!("unknown key `{key}` in `[{id}]`")))?;
                let flag = value
                    .as_bool()
                    .ok_or_else(|| fail(format!("`{key}` in `[{id}]` must be a boolean")))?;
                cfg.set(lang, k, flag);
            }
        }
        Ok(cfg)
    }

    /// Parse a legacy `rustcx.toml`: `max`/`threshold` are top-level, and
    /// `question`/`logical`/`tests` configure the `rust` language. Unknown keys
    /// are errors, as they always were in that file.
    pub fn parse_legacy_rust(
        path: &Path,
        text: &str,
        reg: &Registry,
    ) -> Result<FileConfig, ConfigError> {
        let fail = |message: String| ConfigError {
            path: path.to_path_buf(),
            message,
        };
        let table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| fail(e.to_string()))?;
        let rust = reg.by_id(LEGACY_RUST_FILE.1);
        let mut cfg = FileConfig::default();
        for (key, value) in &table {
            match key.as_str() {
                "max" => {
                    cfg.max = Some(
                        to_u32(value)
                            .ok_or_else(|| fail("`max` must be a non-negative integer".into()))?,
                    )
                }
                "threshold" => {
                    cfg.threshold =
                        Some(to_u32(value).ok_or_else(|| {
                            fail("`threshold` must be a non-negative integer".into())
                        })?)
                }
                _ => {
                    let (lang, k) = rust
                        .and_then(|lang| declared_key(lang, key).map(|k| (lang, k)))
                        .ok_or_else(|| fail(format!("unknown key `{key}`")))?;
                    let flag = value
                        .as_bool()
                        .ok_or_else(|| fail(format!("`{key}` must be a boolean")))?;
                    cfg.set(lang, k, flag);
                }
            }
        }
        Ok(cfg)
    }

    /// Read and parse one file; `rustcx.toml` is read with the legacy mapping.
    fn load(path: &Path, reg: &Registry) -> Result<FileConfig, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        if path
            .file_name()
            .is_some_and(|name| name == LEGACY_RUST_FILE.0)
        {
            FileConfig::parse_legacy_rust(path, &text, reg)
        } else {
            FileConfig::parse(path, &text, reg)
        }
    }

    /// Walk up from `start`, loading the first config found. In each directory
    /// `cmplx.toml` is preferred; `rustcx.toml` is considered only when the
    /// legacy language is registered. `Ok(None)` means no config file.
    pub fn discover(
        start: &Path,
        reg: &Registry,
    ) -> Result<Option<(PathBuf, FileConfig)>, ConfigError> {
        let legacy_enabled = reg.by_id(LEGACY_RUST_FILE.1).is_some();
        for dir in start.ancestors() {
            let found = [FILE_NAME]
                .into_iter()
                .chain(legacy_enabled.then_some(LEGACY_RUST_FILE.0))
                .map(|name| dir.join(name))
                .find(|path| path.is_file());
            if let Some(path) = found {
                let cfg = FileConfig::load(&path, reg)?;
                return Ok(Some((path, cfg)));
            }
        }
        Ok(None)
    }

    /// Build a config from LSP `initializationOptions`: the `cmplx.toml` shape,
    /// plus flat keys that belong to the active languages declaring them. Each
    /// bad key yields one warning and the rest are kept. Tables for registered
    /// but inactive languages are ignored.
    pub fn from_init_options(
        v: &serde_json::Value,
        reg: &Registry,
        active: &[Lang],
    ) -> (FileConfig, Vec<String>) {
        let mut cfg = FileConfig::default();
        let mut warnings = Vec::new();
        let Some(map) = v.as_object() else {
            if !v.is_null() {
                warnings.push("initializationOptions must be an object; ignored".to_string());
            }
            return (cfg, warnings);
        };
        let number = |value: &serde_json::Value| value.as_u64().and_then(|n| u32::try_from(n).ok());
        // Flat keys first so nested tables win, as in `cmplx.toml`.
        for (key, value) in map {
            match key.as_str() {
                "max" | "threshold" => match number(value) {
                    Some(n) if key == "max" => cfg.max = Some(n),
                    Some(n) => cfg.threshold = Some(n),
                    None => warnings.push(format!(
                        "initializationOptions `{key}` must be a non-negative integer; ignored"
                    )),
                },
                _ if reg.by_id(key).is_some() && value.is_object() => {}
                _ => {
                    let owners: Vec<_> = declaring_langs(active, key).collect();
                    match (owners.is_empty(), value.as_bool()) {
                        (true, _) => warnings.push(format!(
                            "unknown initializationOptions key `{key}`; ignored"
                        )),
                        (false, None) => warnings.push(format!(
                            "initializationOptions `{key}` must be a boolean; ignored"
                        )),
                        (false, Some(flag)) => owners
                            .into_iter()
                            .for_each(|(lang, k)| cfg.set(lang, k, flag)),
                    }
                }
            }
        }
        for (id, value) in map {
            let Some(options) = value.as_object().filter(|_| reg.by_id(id).is_some()) else {
                continue;
            };
            let Some(lang) = active.iter().copied().find(|lang| lang.id() == id) else {
                continue;
            };
            for (key, value) in options {
                match (declared_key(lang, key), value.as_bool()) {
                    (None, _) => warnings.push(format!(
                        "unknown initializationOptions key `{key}` in `{id}`; ignored"
                    )),
                    (Some(_), None) => warnings.push(format!(
                        "initializationOptions `{id}.{key}` must be a boolean; ignored"
                    )),
                    (Some(k), Some(flag)) => cfg.set(lang, k, flag),
                }
            }
        }
        (cfg, warnings)
    }

    /// Layer `self` over `lower`: a field set here wins, an unset one falls through.
    pub fn or(self, lower: FileConfig) -> FileConfig {
        let mut langs = lower.langs;
        for (id, options) in self.langs {
            let below = langs.remove(id).unwrap_or_default();
            langs.insert(id, options.or(below));
        }
        FileConfig {
            max: self.max.or(lower.max),
            threshold: self.threshold.or(lower.threshold),
            langs,
        }
    }

    /// The explicitly set options for `lang`.
    pub fn options_for(&self, lang: Lang) -> LangOptions {
        self.langs.get(lang.id()).cloned().unwrap_or_default()
    }

    /// Set one explicit option for `lang` (e.g. from a CLI flag).
    pub fn set_option(&mut self, lang: Lang, key: &'static str, value: bool) {
        self.set(lang, key, value);
    }
}

/// A discovered config file plus the mtime it was read at.
///
/// The language server re-reads on change rather than registering a client file
/// watcher: a `stat` is microseconds against a parse that is milliseconds, and
/// unlike `workspace/didChangeWatchedFiles` it needs no client capability and
/// behaves the same in every editor.
#[derive(Clone)]
pub struct Watched {
    reg: Registry,
    path: Option<PathBuf>,
    mtime: Option<SystemTime>,
    config: FileConfig,
}

impl Watched {
    /// Discover a config file under `start`. Any error is returned alongside a
    /// usable (default) config rather than replacing it, so a bad file at
    /// startup degrades to built-in defaults instead of preventing a session.
    pub fn discover(start: &Path, reg: &Registry) -> (Watched, Option<ConfigError>) {
        let reg = reg.clone();
        match FileConfig::discover(start, &reg) {
            Ok(Some((path, config))) => {
                let mtime = mtime_of(&path);
                (
                    Watched {
                        reg,
                        path: Some(path),
                        mtime,
                        config,
                    },
                    None,
                )
            }
            Ok(None) => (
                Watched {
                    reg,
                    path: None,
                    mtime: None,
                    config: FileConfig::default(),
                },
                None,
            ),
            Err(e) => {
                // Remember the path so a later fix to the file is picked up.
                let path = e.path.clone();
                let mtime = mtime_of(&path);
                (
                    Watched {
                        reg,
                        path: Some(path),
                        mtime,
                        config: FileConfig::default(),
                    },
                    Some(e),
                )
            }
        }
    }

    /// Re-read the file if its mtime changed. Returns an error to log.
    ///
    /// On a parse failure the previous config is kept: reverting to defaults
    /// mid-session would silently change the thresholds someone is working
    /// against. The mtime is still updated, so one bad edit logs once.
    pub fn refresh(&mut self) -> Option<ConfigError> {
        let path = self.path.clone()?;
        let now = mtime_of(&path);
        if now == self.mtime {
            return None;
        }
        self.mtime = now;
        match FileConfig::load(&path, &self.reg) {
            Ok(config) => {
                self.config = config;
                None
            }
            Err(e) => Some(e),
        }
    }

    /// The config as last successfully read.
    pub fn config(&self) -> FileConfig {
        self.config.clone()
    }
}

/// A file's mtime, or `None` if it cannot be stat'd. A missing file compares
/// equal to a missing file, so a deleted config does not re-read every cycle.
fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::{Lang, LangOptions, Language, OptionSpec, Registry};
    use crate::model::{Func, ParseError};
    use std::fs;
    use std::path::{Path, PathBuf};

    const fn spec(key: &'static str) -> OptionSpec {
        OptionSpec {
            key,
            on_flag: None,
            off_flag: None,
            default: None,
            help: "",
        }
    }
    static JS_OPTIONS: [OptionSpec; 2] = [spec("logical"), spec("optional-chain")];
    static RUST_OPTIONS: [OptionSpec; 3] = [spec("logical"), spec("question"), spec("tests")];

    struct FakeLang {
        id: &'static str,
        options: &'static [OptionSpec],
    }
    impl Language for FakeLang {
        fn id(&self) -> &'static str {
            self.id
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
            self.options
        }
        fn skip(&self, _: &Path, _: &LangOptions) -> bool {
            false
        }
        fn analyze(&self, _: &str, _: &Path, _: &LangOptions) -> Result<Vec<Func>, ParseError> {
            Ok(vec![])
        }
    }
    static JS: FakeLang = FakeLang {
        id: "js",
        options: &JS_OPTIONS,
    };
    static RUST: FakeLang = FakeLang {
        id: "rust",
        options: &RUST_OPTIONS,
    };

    fn both() -> Registry {
        Registry::all(&[&JS, &RUST])
    }
    fn js() -> Lang {
        &JS
    }
    fn rust() -> Lang {
        &RUST
    }

    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cmplx-{}-{}-{}", tag, std::process::id(), n));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn parse(text: &str) -> Result<FileConfig, ConfigError> {
        FileConfig::parse(Path::new("cmplx.toml"), text, &both())
    }

    #[test]
    fn an_empty_file_sets_nothing() {
        let cfg = parse("").unwrap();
        assert_eq!(cfg, FileConfig::default());
        assert_eq!(cfg.max, None, "unset is not the same as zero");
    }

    #[test]
    fn every_key_parses() {
        let text = "max = 15\nthreshold = 8\n[js]\noptional-chain = true\n[rust]\nquestion = true\ntests = false\n";
        let cfg = parse(text).unwrap();
        assert_eq!(cfg.max, Some(15));
        assert_eq!(cfg.threshold, Some(8));
        assert_eq!(cfg.options_for(js()).get("optional-chain"), Some(true));
        assert_eq!(cfg.options_for(rust()).get("question"), Some(true));
        assert_eq!(cfg.options_for(rust()).get("tests"), Some(false));
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        let err = parse("treshold = 8").unwrap_err();
        assert!(err.message.contains("treshold"), "got: {}", err.message);
    }

    #[test]
    fn a_wrong_type_is_an_error_naming_key_and_file() {
        for text in [
            "max = \"fifteen\"",
            "threshold = -1",
            "logical = 1",
            "[rust]\ntests = \"no\"",
        ] {
            let err = parse(text).unwrap_err();
            assert!(err.to_string().contains("cmplx.toml"), "{text}: {err}");
            let key = text
                .split([' ', '='])
                .find(|s| !s.is_empty() && !s.starts_with('['))
                .unwrap();
            let key = if text.contains("tests") { "tests" } else { key };
            assert!(err.message.contains(key), "{text}: {}", err.message);
        }
    }

    #[test]
    fn top_level_logical_reaches_every_language_that_declares_it() {
        let cfg = parse("logical = false").unwrap();
        assert_eq!(cfg.options_for(js()).get("logical"), Some(false));
        assert_eq!(cfg.options_for(rust()).get("logical"), Some(false));
    }

    #[test]
    fn a_language_table_overrides_the_top_level() {
        let cfg = parse("logical = false\n[js]\nlogical = true\n").unwrap();
        assert_eq!(cfg.options_for(js()).get("logical"), Some(true));
        assert_eq!(cfg.options_for(rust()).get("logical"), Some(false));
    }

    #[test]
    fn cmplx_rejects_an_unknown_table() {
        let err = parse("[cobol]\nx = true\n").unwrap_err();
        assert!(err.message.contains("cobol"));
        assert!(err.to_string().contains("cmplx.toml"));
    }

    #[test]
    fn an_alias_ignores_tables_for_languages_it_does_not_link() {
        let alias = Registry::alias(&JS);
        let cfg = FileConfig::parse(Path::new("cmplx.toml"), "[rust]\nx = true\n", &alias).unwrap();
        assert_eq!(cfg, FileConfig::default());
    }

    #[test]
    fn an_alias_ignores_top_level_keys_no_linked_language_declares() {
        let alias = Registry::alias(&JS);
        assert!(FileConfig::parse(Path::new("cmplx.toml"), "question = true", &alias).is_ok());
    }

    #[test]
    fn a_misspelled_key_in_a_linked_table_is_an_error_even_in_an_alias() {
        let alias = Registry::alias(&JS);
        let err = FileConfig::parse(
            Path::new("cmplx.toml"),
            "[js]\noptionl-chain = true\n",
            &alias,
        )
        .unwrap_err();
        assert!(err.message.contains("optionl-chain"));
    }

    #[test]
    fn or_prefers_the_left_hand_side() {
        let high = parse("threshold = 20\n[js]\nlogical = false\n").unwrap();
        let low = parse("threshold = 5\nmax = 9\nlogical = true\n").unwrap();
        let merged = high.or(low);
        assert_eq!(merged.threshold, Some(20));
        assert_eq!(merged.max, Some(9));
        assert_eq!(merged.options_for(js()).get("logical"), Some(false));
        assert_eq!(merged.options_for(rust()).get("logical"), Some(true));
    }

    #[test]
    fn discover_finds_a_file_in_a_parent_directory() {
        let root = temp_dir("discover");
        let nested = root.join("src").join("deep");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("cmplx.toml"), "threshold = 7").unwrap();
        let (path, cfg) = FileConfig::discover(&nested, &both())
            .unwrap()
            .expect("walks up");
        assert_eq!(path, root.join("cmplx.toml"));
        assert_eq!(cfg.threshold, Some(7));
    }

    #[test]
    fn discover_returns_none_when_there_is_no_file() {
        let root = temp_dir("nofile");
        assert!(FileConfig::discover(&root, &both()).unwrap().is_none());
    }

    #[test]
    fn rustcx_toml_is_read_as_the_rust_table_when_no_cmplx_toml_exists() {
        let root = temp_dir("legacy");
        fs::write(
            root.join("rustcx.toml"),
            "max = 12\nthreshold = 7\nquestion = true\nlogical = false\ntests = false\n",
        )
        .unwrap();
        let (path, cfg) = FileConfig::discover(&root, &both()).unwrap().unwrap();
        assert_eq!(path, root.join("rustcx.toml"));
        assert_eq!((cfg.max, cfg.threshold), (Some(12), Some(7)));
        let rust_options = cfg.options_for(rust());
        assert_eq!(rust_options.get("question"), Some(true));
        assert_eq!(rust_options.get("logical"), Some(false));
        assert_eq!(rust_options.get("tests"), Some(false));
        assert_eq!(cfg.options_for(js()), LangOptions::default());
    }

    #[test]
    fn rustcx_toml_with_an_unknown_key_is_an_error() {
        let err = FileConfig::parse_legacy_rust(Path::new("rustcx.toml"), "treshold = 8", &both())
            .unwrap_err();
        assert!(err.message.contains("treshold"));
    }

    #[test]
    fn rustcx_toml_is_ignored_when_rust_is_not_registered() {
        let root = temp_dir("legacy-js");
        fs::write(root.join("rustcx.toml"), "threshold = 7").unwrap();
        assert!(FileConfig::discover(&root, &Registry::alias(&JS))
            .unwrap()
            .is_none());
    }

    #[test]
    fn cmplx_toml_wins_over_rustcx_toml_in_the_same_directory() {
        let root = temp_dir("both");
        fs::write(root.join("rustcx.toml"), "threshold = 7").unwrap();
        fs::write(root.join("cmplx.toml"), "threshold = 9").unwrap();
        let (path, cfg) = FileConfig::discover(&root, &both()).unwrap().unwrap();
        assert_eq!(path, root.join("cmplx.toml"));
        assert_eq!(cfg.threshold, Some(9));
    }

    #[test]
    fn flat_init_options_key_belongs_to_the_one_active_language_declaring_it() {
        let v = serde_json::json!({"threshold": 6, "tests": false});
        let (cfg, warnings) = FileConfig::from_init_options(&v, &both(), &[rust()]);
        assert_eq!(cfg.threshold, Some(6));
        assert_eq!(cfg.options_for(rust()).get("tests"), Some(false));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn a_flat_init_key_declared_by_several_active_languages_applies_to_all() {
        let v = serde_json::json!({"logical": false});
        let (cfg, _) = FileConfig::from_init_options(&v, &both(), &[js(), rust()]);
        assert_eq!(cfg.options_for(js()).get("logical"), Some(false));
        assert_eq!(cfg.options_for(rust()).get("logical"), Some(false));
    }

    #[test]
    fn an_unknown_init_option_is_dropped_alone() {
        let v = serde_json::json!({"threshold": 6, "bogus": 1});
        let (cfg, warnings) = FileConfig::from_init_options(&v, &both(), &[rust()]);
        assert_eq!(cfg.threshold, Some(6));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("bogus"));
    }

    #[test]
    fn a_mistyped_init_option_is_dropped_alone() {
        let v = serde_json::json!({"threshold": "six", "tests": false});
        let (cfg, warnings) = FileConfig::from_init_options(&v, &both(), &[rust()]);
        assert_eq!(cfg.threshold, None);
        assert_eq!(cfg.options_for(rust()).get("tests"), Some(false));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("threshold"));
    }

    #[test]
    fn nested_init_options_match_the_toml_shape() {
        let v = serde_json::json!({"rust": {"tests": false}, "js": {"optional-chain": true}});
        let (cfg, warnings) = FileConfig::from_init_options(&v, &both(), &[rust()]);
        assert_eq!(cfg.options_for(rust()).get("tests"), Some(false));
        assert_eq!(
            cfg.options_for(js()),
            LangOptions::default(),
            "inactive language ignored"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn a_nested_init_table_wins_over_a_flat_key() {
        let v = serde_json::json!({"logical": false, "rust": {"logical": true}});
        let (cfg, _) = FileConfig::from_init_options(&v, &both(), &[js(), rust()]);
        assert_eq!(cfg.options_for(rust()).get("logical"), Some(true));
        assert_eq!(cfg.options_for(js()).get("logical"), Some(false));
    }

    #[test]
    fn a_reload_picks_up_a_changed_file() {
        let root = temp_dir("reload");
        let path = root.join("cmplx.toml");
        fs::write(&path, "threshold = 5").unwrap();
        let (mut watched, err) = Watched::discover(&root, &both());
        assert!(err.is_none());
        assert_eq!(watched.config().threshold, Some(5));
        fs::write(&path, "threshold = 30").unwrap();
        bump_mtime(&path);
        assert!(watched.refresh().is_none());
        assert_eq!(watched.config().threshold, Some(30));
    }

    #[test]
    fn a_broken_reload_keeps_the_previous_config_and_logs_once() {
        let root = temp_dir("broken");
        let path = root.join("cmplx.toml");
        fs::write(&path, "threshold = 5").unwrap();
        let (mut watched, _) = Watched::discover(&root, &both());
        fs::write(&path, "threshold = = 5").unwrap();
        bump_mtime(&path);
        let err = watched.refresh().expect("the error is reported");
        assert!(err.to_string().contains("cmplx.toml"));
        assert_eq!(watched.config().threshold, Some(5));
        assert!(watched.refresh().is_none(), "mtime updated so it logs once");
    }

    #[test]
    fn a_bad_file_at_start_yields_defaults_and_an_error_then_recovers() {
        let root = temp_dir("badstart");
        let path = root.join("cmplx.toml");
        fs::write(&path, "threshold = = 5").unwrap();
        let (mut watched, err) = Watched::discover(&root, &both());
        assert!(err.is_some());
        assert_eq!(watched.config(), FileConfig::default());
        fs::write(&path, "threshold = 4").unwrap();
        bump_mtime(&path);
        assert!(watched.refresh().is_none());
        assert_eq!(watched.config().threshold, Some(4));
    }

    #[test]
    fn a_watched_legacy_file_reloads_as_rustcx_toml() {
        let root = temp_dir("watch-legacy");
        let path = root.join("rustcx.toml");
        fs::write(&path, "tests = false").unwrap();
        let (mut watched, err) = Watched::discover(&root, &both());
        assert!(err.is_none());
        fs::write(&path, "tests = true").unwrap();
        bump_mtime(&path);
        assert!(watched.refresh().is_none());
        assert_eq!(
            watched.config().options_for(rust()).get("tests"),
            Some(true)
        );
    }

    fn bump_mtime(path: &Path) {
        let meta = fs::metadata(path).unwrap();
        let later = meta.modified().unwrap() + std::time::Duration::from_secs(60);
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_times(fs::FileTimes::new().set_modified(later))
            .unwrap();
    }
}
