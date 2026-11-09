use zed_extension_api as zed;

struct CmplxExtension;

/// `~/.cargo/bin/cmplx`, built from the worktree shell's `HOME`.
///
/// `which` searches the worktree's `PATH`, which a desktop-launched Zed may
/// not have inherited from a login shell, leaving a cargo-installed binary
/// invisible.
///
/// The path is returned unchecked: the extension runs in a WASI sandbox with no
/// access to arbitrary filesystem paths, so there is no way to stat it from
/// here. A wrong guess surfaces as a spawn failure in Zed's language server
/// log, which is the best this end of the sandbox can do.
fn cargo_bin(worktree: &zed::Worktree) -> Option<String> {
    let home = worktree
        .shell_env()
        .into_iter()
        .find(|(key, _)| key == "HOME")
        .map(|(_, value)| value)?;
    if home.is_empty() {
        return None;
    }
    Some(format!("{home}/.cargo/bin/cmplx"))
}

impl zed::Extension for CmplxExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<zed::Command> {
        let command = worktree
            .which("cmplx")
            .or_else(|| cargo_bin(worktree))
            .ok_or_else(|| {
                "cmplx was not found on PATH or in ~/.cargo/bin. Install it with \
                 'cargo install --path crates/cmplx' from the repository root."
                    .to_string()
            })?;

        Ok(zed::Command {
            command,
            args: vec!["--lsp".to_string()],
            env: Default::default(),
        })
    }

    /// The threshold, only if the user set one.
    ///
    /// Returning `None` rather than a default is load-bearing: the server
    /// resolves `initializationOptions` over `cmplx.toml`, so sending a
    /// default here would override that file in every project.
    fn language_server_initialization_options(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<Option<serde_json::Value>> {
        let threshold = zed::settings::LspSettings::for_worktree("cmplx", worktree)
            .ok()
            .and_then(|s| s.settings)
            .and_then(|s| s.get("threshold").and_then(|v| v.as_u64()))
            .filter(|t| *t > 0);

        Ok(threshold.map(|t| serde_json::json!({ "threshold": t })))
    }
}

zed::register_extension!(CmplxExtension);
