use zed_extension_api as zed;

/// Keep in sync with `jscx::lsp::DEFAULT_THRESHOLD`. The server applies the same
/// default on its own, so this only matters for what we send explicitly.
const DEFAULT_THRESHOLD: u64 = 11;

struct JscxExtension;

impl zed::Extension for JscxExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<zed::Command> {
        let path = worktree.which("jscx").ok_or_else(|| {
            "jscx binary not found in PATH. Please install it with 'cargo install --path .' in the jscx directory.".to_string()
        })?;

        Ok(zed::Command {
            command: path,
            args: vec!["--lsp".to_string()],
            env: Default::default(),
        })
    }

    fn language_server_initialization_options(
        &mut self,
        _server_id: &zed::LanguageServerId,
        _worktree: &zed::Worktree,
    ) -> zed::Result<Option<serde_json::Value>> {
        let settings = zed::settings::LspSettings::for_worktree("jscx", _worktree).ok();
        let threshold = settings
            .and_then(|s| s.settings)
            .and_then(|s| s.get("threshold").and_then(|v| v.as_u64()))
            .filter(|t| *t > 0)
            .unwrap_or(DEFAULT_THRESHOLD);

        Ok(Some(serde_json::json!({
            "threshold": threshold
        })))
    }
}

zed::register_extension!(JscxExtension);
