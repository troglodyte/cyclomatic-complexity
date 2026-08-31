//! `rustcx.toml`, shared by the CLI and the language server.
//!
//! Every field is `Option` so that "unset" is distinguishable from "set to the
//! value that happens to be the default". That distinction is what makes the
//! precedence layering work: without it, a `logical = true` in a file could not
//! be told apart from silence, and a CLI flag could not know whether it was
//! overriding anything.

use crate::analyze::Options;
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The name looked for when walking up from a starting directory.
pub const FILE_NAME: &str = "rustcx.toml";

/// A parsed `rustcx.toml`. Unset fields fall through to the next layer.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    /// CLI: the default for `--max`. Unrelated to `threshold` — see the spec.
    pub max: Option<u32>,
    /// LSP: warn at this score, error at twice it.
    pub threshold: Option<u32>,
    pub question: Option<bool>,
    pub logical: Option<bool>,
    pub tests: Option<bool>,
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

impl FileConfig {
    /// Parse TOML text. Unknown keys are rejected: a typo'd `treshold` that
    /// silently does nothing is a worse failure than a loud one.
    pub fn parse(path: &Path, text: &str) -> Result<FileConfig, ConfigError> {
        toml::from_str(text).map_err(|e| ConfigError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }

    /// Read and parse one file.
    pub fn load(path: &Path) -> Result<FileConfig, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        FileConfig::parse(path, &text)
    }

    /// Layer `self` over `other`: a field set here wins, an unset one falls
    /// through. Used for both `flag > file` and `initializationOptions > file`.
    pub fn or(self, other: FileConfig) -> FileConfig {
        FileConfig {
            max: self.max.or(other.max),
            threshold: self.threshold.or(other.threshold),
            question: self.question.or(other.question),
            logical: self.logical.or(other.logical),
            tests: self.tests.or(other.tests),
        }
    }

    /// Analyzer options with this config layered over the built-in defaults.
    pub fn options(&self) -> Options {
        let d = Options::default();
        Options {
            logical: self.logical.unwrap_or(d.logical),
            question: self.question.unwrap_or(d.question),
            tests: self.tests.unwrap_or(d.tests),
        }
    }

    /// Walk up from `start` to the filesystem root, loading the first
    /// `rustcx.toml` found. `Ok(None)` means there is no config file.
    pub fn discover(start: &Path) -> Result<Option<(PathBuf, FileConfig)>, ConfigError> {
        for dir in start.ancestors() {
            let path = dir.join(FILE_NAME);
            if path.is_file() {
                let cfg = FileConfig::load(&path)?;
                return Ok(Some((path, cfg)));
            }
        }
        Ok(None)
    }
}

/// A discovered config file plus the mtime it was read at.
///
/// The language server re-reads on change rather than registering a client file
/// watcher: a `stat` is microseconds against a parse that is milliseconds, and
/// unlike `workspace/didChangeWatchedFiles` it needs no client capability and
/// behaves the same in every editor.
#[derive(Clone, Debug, Default)]
pub struct Watched {
    path: Option<PathBuf>,
    mtime: Option<SystemTime>,
    config: FileConfig,
}

impl Watched {
    /// Discover a config file under `start`. Any error is returned alongside a
    /// usable (default) config rather than replacing it, so a bad file at
    /// startup degrades to built-in defaults instead of preventing a session.
    pub fn discover(start: &Path) -> (Watched, Option<ConfigError>) {
        match FileConfig::discover(start) {
            Ok(Some((path, config))) => {
                let mtime = mtime_of(&path);
                (Watched { path: Some(path), mtime, config }, None)
            }
            Ok(None) => (Watched::default(), None),
            Err(e) => {
                // Remember the path so a later fix to the file is picked up.
                let path = e.path.clone();
                let mtime = mtime_of(&path);
                (Watched { path: Some(path), mtime, config: FileConfig::default() }, Some(e))
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
        match FileConfig::load(&path) {
            Ok(config) => {
                self.config = config;
                None
            }
            Err(e) => Some(e),
        }
    }

    /// The config as last successfully read.
    pub fn config(&self) -> FileConfig {
        self.config
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
    use std::fs;

    /// A unique empty directory under the system temp dir. No `tempfile`
    /// dependency: one counter plus the pid is enough for a test process.
    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rustcx-{}-{}-{}", tag, std::process::id(), n));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_empty_file_sets_nothing() {
        let cfg = FileConfig::parse(Path::new("rustcx.toml"), "").unwrap();
        assert_eq!(cfg, FileConfig::default());
        assert_eq!(cfg.max, None, "unset is not the same as zero");
    }

    #[test]
    fn every_key_parses() {
        let text = "\
max = 15
threshold = 8
question = true
logical = false
tests = false
";
        let cfg = FileConfig::parse(Path::new("rustcx.toml"), text).unwrap();
        assert_eq!(cfg.max, Some(15));
        assert_eq!(cfg.threshold, Some(8));
        assert_eq!(cfg.question, Some(true));
        assert_eq!(cfg.logical, Some(false));
        assert_eq!(cfg.tests, Some(false));
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        let err = FileConfig::parse(Path::new("rustcx.toml"), "treshold = 8").unwrap_err();
        assert!(
            err.message.contains("treshold") || err.message.contains("unknown"),
            "a typo must be reported, got: {}",
            err.message
        );
    }

    #[test]
    fn a_wrong_type_is_an_error() {
        assert!(FileConfig::parse(Path::new("rustcx.toml"), "max = \"fifteen\"").is_err());
    }

    #[test]
    fn options_layers_over_the_defaults() {
        let d = Options::default();
        assert_eq!(FileConfig::default().options().logical, d.logical);
        assert_eq!(FileConfig::default().options().question, d.question);
        assert_eq!(FileConfig::default().options().tests, d.tests);

        let cfg = FileConfig { tests: Some(false), ..FileConfig::default() };
        assert!(!cfg.options().tests);
        assert!(cfg.options().logical, "an unset key keeps its default");
    }

    #[test]
    fn or_prefers_the_left_hand_side() {
        let high = FileConfig { threshold: Some(20), ..FileConfig::default() };
        let low = FileConfig { threshold: Some(5), max: Some(9), ..FileConfig::default() };
        let merged = high.or(low);
        assert_eq!(merged.threshold, Some(20), "the left side wins where it is set");
        assert_eq!(merged.max, Some(9), "and falls through where it is not");
    }

    #[test]
    fn discover_finds_a_file_in_a_parent_directory() {
        let root = temp_dir("discover");
        let nested = root.join("src").join("deep");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("rustcx.toml"), "threshold = 7").unwrap();

        let (path, cfg) = FileConfig::discover(&nested).unwrap().expect("walks up to the root");
        assert_eq!(path, root.join("rustcx.toml"));
        assert_eq!(cfg.threshold, Some(7));
    }

    #[test]
    fn discover_returns_none_when_there_is_no_file() {
        let root = temp_dir("nofile");
        assert!(FileConfig::discover(&root).unwrap().is_none());
    }

    #[test]
    fn a_reload_picks_up_a_changed_file() {
        let root = temp_dir("reload");
        let path = root.join("rustcx.toml");
        fs::write(&path, "threshold = 5").unwrap();

        let (mut watched, err) = Watched::discover(&root);
        assert!(err.is_none());
        assert_eq!(watched.config().threshold, Some(5));

        // A filesystem mtime can have one-second granularity, so set it
        // explicitly rather than racing it.
        fs::write(&path, "threshold = 30").unwrap();
        bump_mtime(&path);

        assert!(watched.refresh().is_none());
        assert_eq!(watched.config().threshold, Some(30));
    }

    #[test]
    fn a_broken_reload_keeps_the_previous_config() {
        let root = temp_dir("broken");
        let path = root.join("rustcx.toml");
        fs::write(&path, "threshold = 5").unwrap();
        let (mut watched, _) = Watched::discover(&root);

        fs::write(&path, "threshold = = 5").unwrap();
        bump_mtime(&path);

        let err = watched.refresh().expect("the error is reported");
        assert!(err.to_string().contains("rustcx.toml"));
        assert_eq!(
            watched.config().threshold,
            Some(5),
            "a broken edit must not silently revert the thresholds someone is working against"
        );
    }

    /// Push a file's mtime one minute into the future, so a reload is detected
    /// regardless of filesystem timestamp granularity.
    fn bump_mtime(path: &Path) {
        let meta = fs::metadata(path).unwrap();
        let later = meta.modified().unwrap() + std::time::Duration::from_secs(60);
        filetime_set(path, later);
    }

    /// `File::set_times` has been stable since Rust 1.75.
    fn filetime_set(path: &Path, when: std::time::SystemTime) {
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_times(fs::FileTimes::new().set_modified(when)).unwrap();
    }
}
