package dev.rustcx.intellij

import com.intellij.openapi.options.BoundConfigurable
import com.intellij.openapi.project.ProjectManager
import com.intellij.openapi.ui.DialogPanel
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.ui.dsl.builder.bindIntText
import com.intellij.ui.dsl.builder.bindText
import com.intellij.ui.dsl.builder.panel

/** Settings | Tools | rustcx */
internal class RustcxConfigurable : BoundConfigurable("rustcx") {

    private val settings = RustcxSettings.getInstance()

    private var threshold: Int = settings.threshold
    private var binaryPath: String = settings.binaryPath

    override fun createPanel(): DialogPanel = panel {
        row("Complexity threshold:") {
            // 0 is offered on purpose: it is how you hand the decision back to
            // rustcx.toml after having set a value here.
            intTextField(range = 0..10_000)
                .bindIntText(::threshold)
                .comment(
                    "Functions scoring at or above this value are warnings; at or above " +
                        "twice it, errors. Leave at <code>0</code> to use the " +
                        "<code>threshold</code> in <code>rustcx.toml</code>, or the built-in " +
                        "default if there is no such file."
                )
        }
        row("rustcx binary:") {
            textField()
                .bindText(::binaryPath)
                .comment(
                    "Leave blank to resolve <code>rustcx</code> from <code>PATH</code>, " +
                        "falling back to <code>~/.cargo/bin/rustcx</code>."
                )
        }
    }

    override fun apply() {
        super.apply()
        settings.threshold = threshold
        settings.binaryPath = binaryPath.trim()
        restartServers()
    }

    /**
     * The server reads its threshold once, during `initialize`, and implements no
     * `workspace/didChangeConfiguration` handler — so a settings change only takes
     * effect on restart.
     */
    private fun restartServers() {
        for (project in ProjectManager.getInstance().openProjects) {
            if (project.isDisposed) continue
            LspClientManager.getInstance(project)
                .stopAndRestartClientsIfNeeded(RustcxLspIntegrationProvider::class.java)
        }
    }
}
