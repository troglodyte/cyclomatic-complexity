package dev.jscx.intellij

import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspIntegrationProvider

internal class JscxLspIntegrationProvider : LspIntegrationProvider {

    override fun fileOpened(
        project: Project,
        file: VirtualFile,
        clientStarter: LspIntegrationProvider.LspClientStarter,
    ) {
        if (!isSupportedByJscx(file)) return

        val configured = JscxSettings.getInstance().binaryPath
        if (JscxBinary.resolve(configured) == null) {
            JscxNotifications.binaryNotFound(project, configured)
            return
        }

        clientStarter.ensureClientStarted(JscxLspClientDescriptor(project))
    }
}

private val SUPPORTED_EXTENSIONS = setOf("js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts")

private val DECLARATION_SUFFIXES = listOf(".d.ts", ".d.mts", ".d.cts")

/**
 * Whether jscx should score this file.
 *
 * Declaration files are excluded to match the CLI, which skips them unless
 * `--include-dts` is passed. The server analyzes whatever document it is sent,
 * so this filter has to live on the client — otherwise the IDE would report
 * complexity that `jscx` on the command line never shows for the same file.
 */
internal fun isSupportedByJscx(file: VirtualFile): Boolean {
    val name = file.name.lowercase()
    if (DECLARATION_SUFFIXES.any { name.endsWith(it) }) return false
    return file.extension?.lowercase() in SUPPORTED_EXTENSIONS
}
