package dev.rustcx.intellij

import com.intellij.openapi.components.BaseState
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.SimplePersistentStateComponent
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.service

/**
 * Application-wide configuration for the rustcx language server.
 *
 * Application rather than project scope: the threshold is a judgement about what
 * counts as too complex, which does not usually change from one checkout to the
 * next, and this mirrors setting it once in Zed's user settings.
 */
@Service(Service.Level.APP)
@State(name = "RustcxSettings", storages = [Storage("rustcx.xml")])
class RustcxSettings : SimplePersistentStateComponent<RustcxSettings.State>(State()) {

    class State : BaseState() {
        /**
         * Warn at this score, error at twice it. `UNSET` means send nothing and
         * let `rustcx.toml` decide — a real threshold is always at least 1, so
         * zero is free to carry that meaning.
         */
        var threshold: Int by property(UNSET)

        /** Explicit path to the rustcx executable; blank means resolve from PATH. */
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
        /**
         * "No threshold set here." The server's own default lives in
         * `rustcx::lsp::DEFAULT_THRESHOLD` and is deliberately not duplicated:
         * sending a copy of it would override the user's `rustcx.toml`.
         */
        const val UNSET: Int = 0

        fun getInstance(): RustcxSettings = service()
    }
}
