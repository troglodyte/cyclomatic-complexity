package dev.cmplx.intellij

import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspIntegrationProvider

internal class CmplxLspIntegrationProvider : LspIntegrationProvider {

    override fun fileOpened(
        project: Project,
        file: VirtualFile,
        clientStarter: LspIntegrationProvider.LspClientStarter,
    ) {
        if (!isSupportedByCmplx(file)) return

        val configured = CmplxSettings.getInstance().binaryPath
        if (CmplxBinary.resolve(configured) == null) {
            CmplxNotifications.binaryNotFound(project, configured)
            return
        }

        clientStarter.ensureClientStarted(CmplxLspClientDescriptor(project))
    }
}

private val SUPPORTED_EXTENSIONS =
    setOf("js", "mjs", "cjs", "jsx", "ts", "tsx", "mts", "cts", "rs")

/**
 * Whether cmplx scores files with this extension: the JS/TS and Rust lists from
 * `cmplx-js` and `cmplx-rust`. Declaration files (`.d.ts`) are not filtered
 * here; the server skips them.
 */
internal fun isSupportedExtension(extension: String?): Boolean =
    extension?.lowercase() in SUPPORTED_EXTENSIONS

internal fun isSupportedByCmplx(file: VirtualFile): Boolean =
    isSupportedExtension(file.extension)
