package dev.jscx.intellij

import com.intellij.execution.ExecutionException
import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.ProjectWideLspClientDescriptor

/**
 * Runs `jscx --lsp` and hands it the threshold.
 *
 * No feature support is declared beyond diagnostics because the server advertises
 * only `textDocumentSync` in its `initialize` response, and the platform client
 * gates its requests on the server's capabilities. That matters more than it
 * looks: the server ignores methods it does not recognize without replying, so a
 * request for an unsupported feature would never be answered.
 */
internal class JscxLspClientDescriptor(project: Project) :
    ProjectWideLspClientDescriptor(project, "jscx") {

    override fun isSupportedFile(file: VirtualFile): Boolean = isSupportedByJscx(file)

    override fun createCommandLine(): GeneralCommandLine {
        val configured = JscxSettings.getInstance().binaryPath
        val binary = JscxBinary.resolve(configured)
            ?: throw ExecutionException(
                if (configured.isBlank()) {
                    "jscx was not found on PATH. Install it with 'cargo install --path .' " +
                        "or set its location under Settings | Tools | jscx."
                } else {
                    "jscx is not executable at '$configured'. Fix the path under Settings | Tools | jscx."
                }
            )

        return GeneralCommandLine(binary, "--lsp")
    }

    override fun createInitializationOptions(): Any =
        mapOf("threshold" to JscxSettings.getInstance().threshold)
}
