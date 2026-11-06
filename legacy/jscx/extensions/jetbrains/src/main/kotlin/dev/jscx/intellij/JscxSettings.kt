package dev.jscx.intellij

import com.intellij.openapi.components.BaseState
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.SimplePersistentStateComponent
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.service

/**
 * Application-wide configuration for the jscx language server.
 *
 * Application rather than project scope: the threshold is a judgement about what
 * counts as too complex, which does not usually change from one checkout to the
 * next, and this mirrors setting it once in Zed's user settings.
 */
@Service(Service.Level.APP)
@State(name = "JscxSettings", storages = [Storage("jscx.xml")])
class JscxSettings : SimplePersistentStateComponent<JscxSettings.State>(State()) {

    class State : BaseState() {
        /** Warn at this score, error at twice it. */
        var threshold: Int by property(DEFAULT_THRESHOLD)

        /** Explicit path to the jscx executable; blank means resolve from PATH. */
        var binaryPath: String? by string("")
    }

    var threshold: Int
        get() = state.threshold
        set(value) {
            state.threshold = value
        }

    var binaryPath: String
        get() = state.binaryPath.orEmpty()
        set(value) {
            state.binaryPath = value
        }

    companion object {
        /** Keep in sync with `jscx::lsp::DEFAULT_THRESHOLD`. */
        const val DEFAULT_THRESHOLD: Int = 11

        fun getInstance(): JscxSettings = service()
    }
}
