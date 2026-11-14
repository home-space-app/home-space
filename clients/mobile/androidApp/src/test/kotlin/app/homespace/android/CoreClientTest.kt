package app.homespace.android

import kotlin.io.path.createTempDirectory
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.homespace_core.AutoUpload
import uniffi.homespace_core.AutoUploadListener
import uniffi.homespace_core.CoreClient
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.DeviceIdentity
import uniffi.homespace_core.DownloadListener
import uniffi.homespace_core.LocalChanges
import uniffi.homespace_core.LocalItem
import uniffi.homespace_core.LocalSources
import uniffi.homespace_core.SessionListener
import uniffi.homespace_core.SourceKind
import uniffi.homespace_core.ViewerKind
import uniffi.homespace_core.humanSize
import uniffi.homespace_core.normalizeServerUrl
import uniffi.homespace_core.viewerFor

/** The generated Kotlin bindings against the host build of the Rust library. */
class CoreClientTest {
    @Test
    fun formatsSizesInRust() {
        assertEquals("1.5 MB", humanSize(1_572_864))
    }

    @Test
    fun textFilesOpenInTheEditableTextViewer() {
        val info = viewerFor("text/markdown", 10)
        assertEquals(ViewerKind.TEXT, info?.kind)
        assertTrue(info?.editable == true)
    }

    @Test
    fun imagesOpenInTheReadOnlyImageViewer() {
        val info = viewerFor("image/png", 10)
        assertEquals(ViewerKind.IMAGE, info?.kind)
        assertTrue(info?.editable == false)
    }

    @Test
    fun imageSamplingKeepsTheScreenSize() {
        assertEquals(1, sampleSize(1000, 2000))
        assertEquals(2, sampleSize(4000, 2000))
        assertEquals(4, sampleSize(8100, 2000))
    }

    @Test
    fun otherFilesHaveNoViewer() {
        assertEquals(null, viewerFor("application/zip", 10))
    }

    @Test
    fun unreachableServerIsATypedError() = runTest {
        val client = CoreClient("http://127.0.0.1:9")
        try {
            client.health()
            error("expected a failure")
        } catch (e: CoreException.Unreachable) {
            assertTrue(e.message.orEmpty().isNotEmpty())
        } finally {
            client.close()
        }
    }

    @Test
    fun invalidIdIsRejected() = runTest {
        val client = CoreClient("http://127.0.0.1:9")
        try {
            client.list("nope")
            error("expected a failure")
        } catch (e: CoreException.InvalidId) {
            assertEquals("nope", e.id)
        } finally {
            client.close()
        }
    }

    @Test
    fun autoUploadNeedsSomeoneSignedIn() {
        val sources = object : LocalSources {
            override fun changeToken(kind: SourceKind, handle: String): String? = null
            override fun changesSince(kind: SourceKind, handle: String, token: String): LocalChanges? = null
            override fun list(kind: SourceKind, handle: String) = emptyList<LocalItem>()
            override fun stat(kind: SourceKind, handle: String, key: String): LocalItem? = null
            override fun read(
                kind: SourceKind,
                handle: String,
                key: String,
                offset: ULong,
                len: ULong,
            ) = ByteArray(0)
        }
        val listener = object : AutoUploadListener {
            override fun changed() = Unit
        }
        val dir = createTempDirectory().toString()
        val client = CoreClient.withSession(
            "http://127.0.0.1:9",
            DeviceIdentity("00000000-0000-0000-0000-0000000000aa", "Test"),
            object : SessionListener {
                override fun refreshTokenChanged(token: String?) = Unit
            },
            dir,
        )
        client.use {
            assertThrows(CoreException.Unauthorized::class.java) {
                AutoUpload.open(it, dir, sources, listener)
            }
        }
    }

    @Test
    fun sessionListenerIsCalledAcrossTheBindings() = runTest {
        val tokens = mutableListOf<String?>()
        val client = CoreClient.withSession(
            "http://127.0.0.1:9",
            DeviceIdentity("00000000-0000-0000-0000-0000000000aa", "Test"),
            object : SessionListener {
                override fun refreshTokenChanged(token: String?) {
                    tokens.add(token)
                }
            },
            createTempDirectory().toString(),
        )
        try {
            // Signing out with no session needs no network, and still reports the session gone.
            client.logout()
            assertEquals(listOf<String?>(null), tokens)
        } finally {
            client.close()
        }
    }

    @Test
    fun downloadsAreTypedAcrossTheBindings() = runTest {
        val client = CoreClient("http://127.0.0.1:9")
        try {
            client.openCopy(
                "nope",
                object : DownloadListener {
                    override fun progress(id: String, done: Long, total: Long) = Unit
                },
            )
            error("expected a failure")
        } catch (e: CoreException.InvalidId) {
            assertEquals("nope", e.id)
        } finally {
            client.close()
        }
    }

    @Test
    fun beginSsoReturnsTheMobileRedirect() {
        CoreClient("https://files.example.com").use { client ->
            val request = client.beginSso()
            assertTrue(request.url, request.url.contains("redirect_uri=app.homespace"))
            assertTrue(request.verifier.isNotEmpty())
        }
    }

    @Test
    fun serverAddressesAreTidied() {
        assertEquals("https://files.example.com", normalizeServerUrl(" https://files.example.com/ "))
        assertThrows(CoreException.InvalidServerAddress::class.java) {
            normalizeServerUrl("files.example.com")
        }
    }
}
