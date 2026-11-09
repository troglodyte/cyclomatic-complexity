package dev.cmplx.intellij

import com.intellij.execution.ExecutionException
import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.ProjectWideLspClientDescriptor

/**
 * Runs `cmplx --lsp` and hands it the threshold, if one is set.
 *
 * No feature support is declared here: the platform client gates its requests on
 * the capabilities the server advertises in its `initialize` response, which are
 * `textDocumentSync` and `hoverProvider`. If hover does not work in a running
 * IDE, this is the file that needs an explicit override — see the README.
 */
internal class CmplxLspClientDescriptor(project: Project) :
    ProjectWideLspClientDescriptor(project, "cmplx") {

    override fun isSupportedFile(file: VirtualFile): Boolean = isSupportedByCmplx(file)

    override fun createCommandLine(): GeneralCommandLine {
        val configured = CmplxSettings.getInstance().binaryPath
        val binary = CmplxBinary.resolve(configured)
            ?: throw ExecutionException(
                if (configured.isBlank()) {
                    "cmplx was not found on PATH. Install it with 'cargo install --path crates/cmplx' " +
                        "or set its location under Settings | Tools | cmplx."
                } else {
                    "cmplx is not executable at '$configured'. Fix the path under Settings | Tools | cmplx."
                }
            )

        return GeneralCommandLine(binary, "--lsp")
    }

    /**
     * `null` when no threshold is set, so the server falls back to `cmplx.toml`
     * — `Server::settings` resolves initialization options *over* the file, so a
     * default sent from here would silently disable that key everywhere.
     */
    override fun createInitializationOptions(): Any? =
        CmplxSettings.getInstance().threshold
            .takeIf { it != CmplxSettings.UNSET }
            ?.let { mapOf("threshold" to it) }
}
