package dev.rustcx.intellij

import com.intellij.execution.configurations.PathEnvironmentVariableUtil
import java.io.File
import java.nio.file.Path

/** Locates the `rustcx` executable. */
object RustcxBinary {

    const val EXECUTABLE: String = "rustcx"

    /**
     * Resolution order: the configured path, then `PATH`, then Cargo's bin
     * directory.
     *
     * The Cargo fallback is not redundant. An IDE launched from the Dock or the
     * Start menu does not inherit the shell's `PATH`, so `~/.cargo/bin` is
     * routinely invisible to the second step even though the binary is installed
     * and works fine in a terminal.
     *
     * A configured path that is not executable resolves to `null` rather than
     * silently falling through, so a typo surfaces as an error instead of as
     * mysteriously different behavior.
     *
     * @param pathLookup injected for testing; defaults to the platform's `PATH` search
     * @param home injected for testing; defaults to the user's home directory
     */
    fun resolve(
        configured: String,
        // findExecutableInPathOnAnyOS, rather than findInPath, so Windows finds rustcx.exe.
        pathLookup: (String) -> File? = { PathEnvironmentVariableUtil.findExecutableInPathOnAnyOS(it) },
        home: Path = Path.of(System.getProperty("user.home")),
    ): String? {
        val explicit = configured.trim()
        if (explicit.isNotEmpty()) {
            return explicit.takeIf { File(it).canExecute() }
        }

        pathLookup(EXECUTABLE)?.let { found ->
            if (found.canExecute()) return found.absolutePath
        }

        val cargo = home.resolve(".cargo").resolve("bin").resolve(executableName()).toFile()
        return cargo.absolutePath.takeIf { cargo.canExecute() }
    }

    private fun executableName(): String =
        if (isWindows()) "$EXECUTABLE.exe" else EXECUTABLE

    private fun isWindows(): Boolean =
        System.getProperty("os.name").orEmpty().startsWith("Windows", ignoreCase = true)
}
