//! Turning command-line paths into source files, each paired with its language.

use crate::config::FileConfig;
use crate::language::{Lang, Registry};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Directories never walked into: build output, dependencies, VCS metadata and
/// virtual environments. The union of jscx's and rustcx's lists, plus Python's.
pub const SKIP_DIRS: &[&str] = &[
    ".cache",
    ".git",
    ".hg",
    ".jj",
    ".next",
    ".nuxt",
    ".output",
    ".svelte-kit",
    ".svn",
    ".turbo",
    ".venv",
    "__pycache__",
    "build",
    "coverage",
    "dist",
    "node_modules",
    "out",
    "target",
    "vendor",
    "venv",
];

/// A file to score and the language that scores it.
#[derive(Clone)]
pub struct Source {
    pub path: PathBuf,
    pub lang: Lang,
}

/// Which languages may claim files, and the options they skip files by.
pub struct Scope<'a> {
    pub reg: &'a Registry,
    pub active: &'a [Lang],
    /// An alias or language flag chose `active` (see `Cli::explicit_lang`).
    pub explicit_lang: bool,
    pub config: &'a FileConfig,
}

impl Scope<'_> {
    /// The language for a file found while walking a directory: one whose
    /// extension matches and that does not skip it.
    fn walked(&self, path: &Path) -> Option<Lang> {
        let lang = self.reg.for_path(path, self.active)?;
        let skipped = lang.skip(path, &self.config.options_for(lang));
        (!skipped).then_some(lang)
    }

    /// The language for a file named on the command line. With exactly one
    /// chosen language, that language takes it whatever its extension.
    fn named(&self, path: &Path) -> Option<Lang> {
        match self.active {
            [only] if self.explicit_lang => Some(*only),
            _ => self.reg.for_path(path, self.active),
        }
    }

    fn language_flags(&self) -> String {
        self.reg
            .langs()
            .iter()
            .map(|lang| format!("--{}", lang.id()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The sources found under the command-line paths, plus one message per path
/// that could not be used (unreadable, or claimed by no language).
pub struct Expanded {
    pub sources: Vec<Source>,
    pub problems: Vec<String>,
}

/// Expand command-line paths: a directory is walked for files the active
/// languages claim; a named file is taken as given (see [`Scope::named`]).
pub fn expand(paths: &[PathBuf], scope: &Scope) -> Expanded {
    let mut out = Expanded {
        sources: Vec::new(),
        problems: Vec::new(),
    };
    for path in paths {
        let meta = match fs::metadata(path) {
            Ok(meta) => meta,
            Err(e) => {
                out.problems.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        if meta.is_dir() {
            if let Err(e) = collect(path, scope, &mut out.sources) {
                out.problems.push(format!("{}: {e}", path.display()));
            }
            continue;
        }
        match scope.named(path) {
            Some(lang) => out.sources.push(Source {
                path: path.clone(),
                lang,
            }),
            None => out.problems.push(format!(
                "no language for {}; pass {}",
                path.display(),
                scope.language_flags()
            )),
        }
    }
    out
}

/// Recursively collect claimed files under `root` in sorted order, skipping
/// [`SKIP_DIRS`].
fn collect(root: &Path, scope: &Scope, out: &mut Vec<Source>) -> io::Result<()> {
    let mut entries: Vec<PathBuf> = fs::read_dir(root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            let skip = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| SKIP_DIRS.contains(&n));
            if !skip {
                collect(&path, scope, out)?;
            }
        } else if let Some(lang) = scope.walked(&path) {
            out.push(Source { path, lang });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FileConfig;
    use crate::language::{LangOptions, Language, OptionSpec, Registry};
    use crate::model::{Func, ParseError};
    use std::fs;
    use std::path::{Path, PathBuf};

    struct FakeLang {
        id: &'static str,
        extensions: &'static [&'static str],
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
            self.extensions
        }
        fn stdin_ext(&self) -> &'static str {
            self.extensions[0]
        }
        fn options(&self) -> &'static [OptionSpec] {
            &[]
        }
        /// Skips `skipme.*` unless the `all` option is set.
        fn skip(&self, path: &Path, opts: &LangOptions) -> bool {
            opts.get("all") != Some(true)
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("skipme."))
        }
        fn analyze(&self, _: &str, _: &Path, _: &LangOptions) -> Result<Vec<Func>, ParseError> {
            Ok(vec![])
        }
    }
    static JS: FakeLang = FakeLang {
        id: "js",
        extensions: &["js", "ts"],
    };
    static RUST: FakeLang = FakeLang {
        id: "rust",
        extensions: &["rs"],
    };

    fn both() -> Registry {
        Registry::all(&[&JS, &RUST])
    }

    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("cmplx-walk-{}-{}-{}", tag, std::process::id(), n));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(root: &Path, rel: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    fn expand_with(
        paths: &[PathBuf],
        reg: &Registry,
        active: &[Lang],
        explicit_lang: bool,
        config: &FileConfig,
    ) -> Expanded {
        let scope = Scope {
            reg,
            active,
            explicit_lang,
            config,
        };
        expand(paths, &scope)
    }

    /// `(path relative to root, language id)` for each source found.
    fn found(root: &Path, expanded: &Expanded) -> Vec<(String, &'static str)> {
        expanded
            .sources
            .iter()
            .map(|s| {
                let rel = s.path.strip_prefix(root).unwrap_or(&s.path);
                (rel.display().to_string(), s.lang.id())
            })
            .collect()
    }

    #[test]
    fn a_walk_gives_each_file_to_the_language_claiming_its_extension_in_sorted_order() {
        let root = temp_dir("claims");
        for rel in ["b.rs", "a.ts", "sub/c.js", "notes.txt", "README"] {
            touch(&root, rel);
        }
        let reg = both();
        let out = expand_with(
            std::slice::from_ref(&root),
            &reg,
            reg.langs(),
            false,
            &FileConfig::default(),
        );
        assert_eq!(
            found(&root, &out),
            [
                ("a.ts".into(), "js"),
                ("b.rs".into(), "rust"),
                ("sub/c.js".into(), "js")
            ]
        );
        assert!(out.problems.is_empty(), "{:?}", out.problems);
    }

    #[test]
    fn a_walk_only_takes_active_languages() {
        let root = temp_dir("active");
        touch(&root, "a.ts");
        touch(&root, "b.rs");
        let out = expand_with(
            std::slice::from_ref(&root),
            &both(),
            &[&RUST],
            true,
            &FileConfig::default(),
        );
        assert_eq!(found(&root, &out), [("b.rs".into(), "rust")]);
    }

    #[test]
    fn build_vcs_and_environment_directories_are_skipped() {
        let root = temp_dir("skips");
        for dir in [
            "node_modules",
            "target",
            ".git",
            "dist",
            "build",
            "coverage",
            ".next",
            "__pycache__",
            ".venv",
            "venv",
            "vendor",
            ".jj",
        ] {
            touch(&root, &format!("{dir}/x.rs"));
        }
        touch(&root, "kept/x.rs");
        let reg = both();
        let out = expand_with(
            std::slice::from_ref(&root),
            &reg,
            reg.langs(),
            false,
            &FileConfig::default(),
        );
        assert_eq!(found(&root, &out), [("kept/x.rs".into(), "rust")]);
    }

    #[test]
    fn a_language_can_skip_files_during_a_walk_according_to_its_options() {
        let root = temp_dir("lang-skip");
        touch(&root, "skipme.ts");
        touch(&root, "kept.ts");
        let reg = both();
        let out = expand_with(
            std::slice::from_ref(&root),
            &reg,
            reg.langs(),
            false,
            &FileConfig::default(),
        );
        assert_eq!(found(&root, &out), [("kept.ts".into(), "js")]);

        let mut config = FileConfig::default();
        config.set_option(&JS, "all", true);
        let out = expand_with(
            std::slice::from_ref(&root),
            &reg,
            reg.langs(),
            false,
            &config,
        );
        assert_eq!(found(&root, &out).len(), 2);
    }

    #[test]
    fn a_named_file_is_never_skipped() {
        let root = temp_dir("named-skip");
        touch(&root, "skipme.ts");
        let reg = both();
        let out = expand_with(
            &[root.join("skipme.ts")],
            &reg,
            reg.langs(),
            false,
            &FileConfig::default(),
        );
        assert_eq!(found(&root, &out), [("skipme.ts".into(), "js")]);
    }

    #[test]
    fn a_named_file_under_one_explicit_language_is_scored_as_it_whatever_its_extension() {
        let root = temp_dir("explicit");
        touch(&root, "build.txt");
        let out = expand_with(
            &[root.join("build.txt")],
            &Registry::alias(&RUST),
            &[&RUST],
            true,
            &FileConfig::default(),
        );
        assert_eq!(found(&root, &out), [("build.txt".into(), "rust")]);
        assert!(out.problems.is_empty());
    }

    #[test]
    fn a_named_file_no_language_claims_is_a_problem_naming_the_language_flags() {
        let root = temp_dir("unclaimed");
        touch(&root, "notes.txt");
        let path = root.join("notes.txt");
        let reg = both();
        let out = expand_with(
            std::slice::from_ref(&path),
            &reg,
            reg.langs(),
            false,
            &FileConfig::default(),
        );
        assert!(out.sources.is_empty());
        assert_eq!(
            out.problems,
            [format!(
                "no language for {}; pass --js, --rust",
                path.display()
            )]
        );
    }

    #[test]
    fn several_explicit_languages_still_need_an_extension_match() {
        let root = temp_dir("two-explicit");
        touch(&root, "notes.txt");
        touch(&root, "a.rs");
        let reg = both();
        let paths = [root.join("notes.txt"), root.join("a.rs")];
        let out = expand_with(&paths, &reg, reg.langs(), true, &FileConfig::default());
        assert_eq!(found(&root, &out), [("a.rs".into(), "rust")]);
        assert_eq!(out.problems.len(), 1);
    }

    #[test]
    fn a_missing_path_is_a_problem_naming_it() {
        let root = temp_dir("missing");
        let path = root.join("does-not-exist.rs");
        let reg = both();
        let out = expand_with(
            std::slice::from_ref(&path),
            &reg,
            reg.langs(),
            false,
            &FileConfig::default(),
        );
        assert!(out.sources.is_empty());
        assert_eq!(out.problems.len(), 1);
        assert!(
            out.problems[0].starts_with(&format!("{}: ", path.display())),
            "{:?}",
            out.problems
        );
    }
}
