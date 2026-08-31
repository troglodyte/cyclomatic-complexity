package dev.rustcx.intellij

import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspIntegrationProvider

internal class RustcxLspIntegrationProvider : LspIntegrationProvider {

    override fun fileOpened(
        project: Project,
        file: VirtualFile,
        clientStarter: LspIntegrationProvider.LspClientStarter,
    ) {
        if (!isSupportedByRustcx(file)) return

        val configured = RustcxSettings.getInstance().binaryPath
        if (RustcxBinary.resolve(configured) == null) {
            RustcxNotifications.binaryNotFound(project, configured)
            return
        }

        clientStarter.ensureClientStarted(RustcxLspClientDescriptor(project))
    }
}

/**
 * Whether rustcx should score this file.
 *
 * Just the extension. jscx needs a richer filter because its CLI skips `.d.ts`
 * declaration files while its server scores whatever arrives; rustcx has no such
 * asymmetry, so there is nothing here but `.rs`.
 */
internal fun isSupportedByRustcx(file: VirtualFile): Boolean =
    file.extension?.lowercase() == "rs"
