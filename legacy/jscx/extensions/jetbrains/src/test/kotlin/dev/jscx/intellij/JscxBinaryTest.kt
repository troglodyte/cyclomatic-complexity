package dev.jscx.intellij

import java.io.File
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class JscxBinaryTest {

    private fun executable(dir: Path, name: String = "jscx"): File {
        Files.createDirectories(dir)
        val file = dir.resolve(name).toFile()
        file.writeText("#!/bin/sh\nexit 0\n")
        check(file.setExecutable(true)) { "could not mark $file executable" }
        return file
    }

    @Test
    fun `configured path wins over PATH`() {
        val home = Files.createTempDirectory("home")
        val explicit = executable(Files.createTempDirectory("explicit"))
        val onPath = executable(Files.createTempDirectory("onpath"))

        val resolved = JscxBinary.resolve(explicit.absolutePath, { onPath }, home)

        assertEquals(explicit.absolutePath, resolved)
    }

    @Test
    fun `configured path is trimmed`() {
        val home = Files.createTempDirectory("home")
        val explicit = executable(Files.createTempDirectory("explicit"))

        val resolved = JscxBinary.resolve("  ${explicit.absolutePath}  ", { null }, home)

        assertEquals(explicit.absolutePath, resolved)
    }

    @Test
    fun `a configured path that is not executable does not fall through to PATH`() {
        val home = Files.createTempDirectory("home")
        val onPath = executable(Files.createTempDirectory("onpath"))

        val resolved = JscxBinary.resolve("/nope/jscx", { onPath }, home)

        assertNull(resolved, "a typo must surface as an error, not as silent fallback")
    }

    @Test
    fun `blank configuration resolves from PATH`() {
        val home = Files.createTempDirectory("home")
        val onPath = executable(Files.createTempDirectory("onpath"))

        val resolved = JscxBinary.resolve("   ", { onPath }, home)

        assertEquals(onPath.absolutePath, resolved)
    }

    @Test
    fun `falls back to the cargo bin directory when PATH has nothing`() {
        val home = Files.createTempDirectory("home")
        val cargo = executable(home.resolve(".cargo").resolve("bin"))

        val resolved = JscxBinary.resolve("", { null }, home)

        assertEquals(cargo.absolutePath, resolved)
    }

    @Test
    fun `resolves to null when the binary is nowhere`() {
        val home = Files.createTempDirectory("home")

        val resolved = JscxBinary.resolve("", { null }, home)

        assertNull(resolved)
    }

    @Test
    fun `a non-executable file on PATH is not accepted`() {
        val home = Files.createTempDirectory("home")
        val plain = Files.createTempDirectory("onpath").resolve("jscx").toFile()
        plain.writeText("not executable")
        check(plain.setExecutable(false))

        val resolved = JscxBinary.resolve("", { plain }, home)

        assertNull(resolved)
    }
}
