package dev.cmplx.intellij

import java.io.File
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

/**
 * Resolution is pure apart from the two dependencies it takes as parameters, so
 * these tests need no IDE fixture — only real files, because `canExecute` is the
 * one thing that cannot be faked.
 */
class CmplxBinaryTest {

    private val temp: Path = Files.createTempDirectory("cmplx-binary-test")

    @AfterTest
    fun cleanUp() {
        temp.toFile().deleteRecursively()
    }

    /** A real, executable file at [relative], and its absolute path. */
    private fun executable(relative: String): String {
        val file = temp.resolve(relative).toFile()
        file.parentFile.mkdirs()
        file.writeText("#!/bin/sh\n")
        check(file.setExecutable(true)) { "could not mark $file executable" }
        return file.absolutePath
    }

    @Test
    fun `a configured path wins`() {
        val configured = executable("custom/cmplx")
        assertEquals(
            configured,
            CmplxBinary.resolve(configured, pathLookup = { null }, home = temp),
        )
    }

    @Test
    fun `a configured path that is not executable resolves to null`() {
        // Not a fallthrough: a typo must surface as an error, not as PATH's copy.
        val onPath = executable("bin/cmplx")
        assertNull(
            CmplxBinary.resolve(
                temp.resolve("nope/cmplx").toString(),
                pathLookup = { File(onPath) },
                home = temp,
            )
        )
    }

    @Test
    fun `PATH is used when nothing is configured`() {
        val onPath = executable("bin/cmplx")
        assertEquals(
            onPath,
            CmplxBinary.resolve("", pathLookup = { File(onPath) }, home = temp),
        )
    }

    @Test
    fun `blank and whitespace configured paths are treated as unset`() {
        val onPath = executable("bin/cmplx")
        assertEquals(
            onPath,
            CmplxBinary.resolve("   ", pathLookup = { File(onPath) }, home = temp),
        )
    }

    @Test
    fun `the cargo bin is the fallback when PATH has no hit`() {
        val cargo = executable(".cargo/bin/cmplx")
        assertEquals(
            cargo,
            CmplxBinary.resolve("", pathLookup = { null }, home = temp),
        )
    }

    @Test
    fun `a non-executable PATH hit falls through to the cargo bin`() {
        val cargo = executable(".cargo/bin/cmplx")
        val plainFile = temp.resolve("bin/plain").toFile()
        plainFile.parentFile.mkdirs()
        plainFile.writeText("not executable")
        plainFile.setExecutable(false)

        assertEquals(
            cargo,
            CmplxBinary.resolve("", pathLookup = { plainFile }, home = temp),
        )
    }

    @Test
    fun `nothing anywhere resolves to null`() {
        assertNull(CmplxBinary.resolve("", pathLookup = { null }, home = temp))
    }

    @Test
    fun `the executable name is the plain binary name`() {
        assertEquals("cmplx", CmplxBinary.EXECUTABLE)
    }
}
