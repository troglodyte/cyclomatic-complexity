package dev.cmplx.intellij

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/** Keep in sync with `Language::extensions` in `cmplx-js` and `cmplx-rust`. */
class CmplxSupportedFilesTest {

    @Test
    fun `every JavaScript and TypeScript extension is supported`() {
        listOf("js", "mjs", "cjs", "jsx", "ts", "tsx", "mts", "cts").forEach {
            assertTrue(isSupportedExtension(it), it)
        }
    }

    @Test
    fun `Rust is supported`() {
        assertTrue(isSupportedExtension("rs"))
    }

    @Test
    fun `extension matching ignores case`() {
        assertTrue(isSupportedExtension("TSX"))
    }

    @Test
    fun `other and missing extensions are not supported`() {
        assertFalse(isSupportedExtension("py"))
        assertFalse(isSupportedExtension(null))
    }
}
