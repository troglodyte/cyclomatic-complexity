package dev.jscx.intellij

import com.intellij.openapi.options.BoundConfigurable
import com.intellij.openapi.project.ProjectManager
import com.intellij.openapi.ui.DialogPanel
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.ui.dsl.builder.bindIntText
import com.intellij.ui.dsl.builder.bindText
import com.intellij.ui.dsl.builder.panel

/** Settings | Tools | jscx */
internal class JscxConfigurable : BoundConfigurable("jscx") {

    private val settings = JscxSettings.getInstance()

    private var threshold: Int = settings.threshold
    private var binaryPath: String = settings.binaryPath

    override fun createPanel(): DialogPanel = panel {
        row("Complexity threshold:") {
            intTextField(range = 1..10_000)
                .bindIntText(::threshold)
                .comment(
                    "Functions scoring at or above this value are warnings; at or above " +
                        "twice it, errors. Default is ${JscxSettings.DEFAULT_THRESHOLD}."
                )
        }
        row("jscx binary:") {
            textField()
                .bindText(::binaryPath)
                .comment(
                    "Leave blank to resolve <code>jscx</code> from <code>PATH</code>, " +
                        "falling back to <code>~/.cargo/bin/jscx</code>."
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
                .stopAndRestartClientsIfNeeded(JscxLspIntegrationProvider::class.java)
        }
    }
}
