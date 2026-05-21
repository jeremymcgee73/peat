package com.defenseunicorns.peat

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Surface-tier instrumented tests for the peat-ffi JNI surface
 * shipped in `com.defenseunicorns:peat-ffi`.
 *
 * The Rust unit-test suite in `peat-ffi/src/lib.rs` covers the
 * helper logic (JSON serialization, AutomergeBackend setup, …) but
 * can't exercise the JNI bridge itself — argument marshalling, the
 * env.throw_new ↔ Kotlin RuntimeException propagation, native lib
 * loading + RegisterNatives, classloader interactions. Those
 * surface-tier behaviors only matter at runtime against a real JVM.
 *
 * Runs on the self-hosted `peat-arm64-linux-gb10` runner via the
 * `android-test` workflow. Test count is intentionally minimal —
 * peat-mesh#145 (M4b) carries the full two-backend sync surface
 * coverage. This file's job is to lock in the specific JNI-bridge
 * contracts that consumers depend on and that peat-mesh#145's QA
 * round identified as untested.
 *
 * peat#888 — surface-tier coverage gate.
 */
@RunWith(AndroidJUnit4::class)
class PeatJniSurfaceTest {

    companion object {
        // Base64 of "test-key-1234567890123456789012345678901234".
        // Test fixture — same value used in peat-mesh#145's
        // SyncProtocolTest. Both backends in a multi-node test must
        // share the key so their derived formation keys match.
        private const val SHARED_KEY = "dGVzdC1rZXktMTIzNDU2Nzg5MDEyMzQ1Njc4OTAxMjM0"
        private const val APP_ID = "peat-ffi-surface-test"
    }

    private val handles = mutableListOf<Long>()
    private val storageDirs = mutableListOf<File>()

    @After
    fun teardown() {
        handles.forEach { handle ->
            try {
                PeatJni.freeNodeJni(handle)
            } catch (t: Throwable) {
                // Best-effort cleanup; don't mask test failures.
            }
        }
        handles.clear()
        storageDirs.forEach { it.deleteRecursively() }
        storageDirs.clear()
    }

    private fun createNode(label: String): Long {
        val cacheDir = InstrumentationRegistry.getInstrumentation()
            .targetContext
            .cacheDir
        val storageDir = File(cacheDir, "peat-ffi-surface-$label-${System.nanoTime()}").apply {
            mkdirs()
        }
        storageDirs.add(storageDir)
        val handle = PeatJni.createNodeJni(APP_ID, SHARED_KEY, storageDir.absolutePath)
        assertTrue(
            "createNodeJni returned 0 for $label (check logcat for PeatRust tag)",
            handle != 0L,
        )
        handles.add(handle)
        return handle
    }

    /**
     * peat#887 regression gate: `System.loadLibrary("peat_ffi")`
     * must not crash on consumers that don't ship a custom
     * `PeerEventManager`. The PeerEventManager.kt stub now shipped
     * in the AAR (peat#886) plus the env.exception_clear fix in
     * JNI_OnLoad together make this safe.
     *
     * Reaching `PeatJni.peatVersion()` here means:
     * - peat_ffi.so loaded
     * - JNI_OnLoad survived (the find_class("PeerEventManager") +
     *   find_class("PeatJni") + RegisterNatives chain didn't abort)
     * - PeatJni.<clinit> ran (nativeInit() returned cleanly)
     * - PeerEventManager class was reachable (the AAR-shipped stub)
     *
     * Pre-0.1.2 (with peat#887 unfixed and PeerEventManager not in
     * the AAR), this test would never have gotten this far —
     * SIGABRT at System.loadLibrary kills the test process.
     */
    @Test
    fun loadLibrarySucceeds_andNoConsumerPeerEventManagerRequired() {
        val version = PeatJni.peatVersion()
        assertTrue("peatVersion must return non-empty string", version.isNotEmpty())
    }

    /**
     * peat#885 contract test: `forceStoreErrorForTestingJni` arms a
     * one-shot flag; the next `getDocumentJni` throws
     * `RuntimeException`; subsequent calls return normally (flag
     * self-cleared). This is the deterministic verification of the
     * `Err(_) → env.throw_new` propagation that peat-mesh#145's
     * cache-eviction approach couldn't fail-closed on.
     *
     * Steps:
     *   1. Create node.
     *   2. Arm the fault: `forceStoreErrorForTestingJni(handle)`
     *      returns true.
     *   3. First `getDocumentJni` call throws RuntimeException.
     *   4. Second `getDocumentJni` call returns null (flag cleared,
     *      doc never existed).
     */
    @Test
    fun forceStoreErrorForTesting_throwsOnce_thenClears() {
        val handle = createNode("err-fixture")
        val collection = "platforms"
        val docId = "anything"

        // Step 2: arm.
        assertTrue(
            "forceStoreErrorForTestingJni must return true on a valid handle",
            PeatJni.forceStoreErrorForTestingJni(handle),
        )

        // Step 3: first getDocumentJni throws RuntimeException.
        try {
            val result = PeatJni.getDocumentJni(handle, collection, docId)
            fail(
                "expected RuntimeException after arming fault injection; " +
                    "got result=$result instead",
            )
        } catch (e: RuntimeException) {
            // peat-ffi's env.throw_new uses a message prefix that's
            // part of the wrapper contract. Pin it loosely so a
            // future message reword doesn't break the test, but
            // catches the case where some OTHER RuntimeException
            // happens to slip through.
            assertNotNull("exception message must not be null", e.message)
            assertTrue(
                "RuntimeException message should mention getDocumentJni; got: ${e.message}",
                e.message?.contains("getDocumentJni") == true,
            )
        }

        // Step 4: second call clears flag → returns null (doc never
        // existed, but no throw). Validates single-shot semantics.
        val secondResult = PeatJni.getDocumentJni(handle, collection, docId)
        assertNull(
            "second getDocumentJni after one-shot consumption must return null " +
                "(doc never existed; flag self-cleared)",
            secondResult,
        )
    }

    /**
     * peat#885 boundary: invalid handle (0) must return false from
     * forceStoreErrorForTestingJni without arming the flag. Prevents
     * a stale-handle call from leaving the next legitimate
     * getDocumentJni in a forced-throw state.
     */
    @Test
    fun forceStoreErrorForTesting_invalidHandle_doesNotArm() {
        assertEquals(
            "forceStoreErrorForTestingJni(0) must return false",
            false,
            PeatJni.forceStoreErrorForTestingJni(0L),
        )

        // Create a fresh node; getDocumentJni should NOT throw
        // because the prior invalid-handle call didn't arm the flag.
        val handle = createNode("invalid-arm-fixture")
        val result = PeatJni.getDocumentJni(handle, "platforms", "anything")
        assertNull(
            "getDocumentJni must not be in armed state after a 0-handle arm attempt",
            result,
        )
    }

    /**
     * peat#886 sanity check: the canonical PeatJni.kt shipped in
     * the AAR exposes every method this test suite — and any
     * consumer — calls. If a future refactor renames or removes
     * any of them, this test fails at compile time — not runtime —
     * giving consumers a PR-gate signal for binding drift.
     *
     * Coverage spans the entire PeatJni surface (lifecycle, peer
     * state, sync, generic doc I/O, typed-collection accessors,
     * blob transfer, BLE state, and the test-only fault injector).
     * Methods that take consumer-supplied listener interfaces
     * (subscribeDocumentChangesJni / subscribeOutboundFramesJni
     * and their unsubscribe pairs) are declared outside this object
     * per the doc-comment in PeatJni.kt and don't appear here.
     *
     * The test body is a no-op; the value is in the references.
     */
    @Test
    fun peatJniCanonicalSurface_compileTimeReferences() {
        // Touch every method the AAR commits to. If any of these
        // gets renamed or its signature changes, the AAR consumer
        // (this test) won't compile against the new AAR.
        val refs: Array<Any> = arrayOf(
            // Lifecycle
            ::peatJniRefNativeInit,
            ::peatJniRefPeatVersion,
            ::peatJniRefTestJni,
            ::peatJniRefCreateNode,
            ::peatJniRefCreateNodeWithConfig,
            ::peatJniRefGetGlobalNodeHandle,
            ::peatJniRefFreeNode,
            // Peer state
            ::peatJniRefNodeId,
            ::peatJniRefPeerCount,
            ::peatJniRefConnectedPeers,
            ::peatJniRefEndpointSocketAddr,
            ::peatJniRefConnectPeer,
            // Sync coordination
            ::peatJniRefStartSync,
            ::peatJniRefRequestSync,
            // Generic document I/O
            ::peatJniRefPublishDocument,
            ::peatJniRefPublishDocumentWithOrigin,
            ::peatJniRefGetDocument,
            // Typed collection accessors
            ::peatJniRefGetCells,
            ::peatJniRefGetTracks,
            ::peatJniRefGetPlatforms,
            ::peatJniRefGetCommands,
            ::peatJniRefGetMarkers,
            ::peatJniRefPublishPlatform,
            ::peatJniRefPublishMarker,
            ::peatJniRefIngestPosition,
            // Blob transfer
            ::peatJniRefEnableBlobTransfer,
            ::peatJniRefBlobAddPeer,
            ::peatJniRefBlobPut,
            ::peatJniRefBlobGet,
            ::peatJniRefBlobExistsLocally,
            ::peatJniRefBlobEndpointId,
            // BLE transport state
            ::peatJniRefBleSetStarted,
            ::peatJniRefBleAddPeer,
            ::peatJniRefBleRemovePeer,
            ::peatJniRefBleIsAvailable,
            ::peatJniRefBlePeerCount,
            // Test-only fault injection
            ::peatJniRefForceStoreError,
        )
        // 37 PeatJni methods total. If this number changes, the
        // count below must change too — and the new method needs
        // its own peatJniRef* shim added above.
        assertEquals(37, refs.size)
    }

    // -- Reference shims --------------------------------------------------
    // Each shim calls a PeatJni method so the method's signature is
    // captured at compile time. Body content doesn't matter — the
    // goal is the static reference. None of these are ever invoked
    // (they're @Suppress'd; the array above only takes function refs).

    // Lifecycle
    @Suppress("unused")
    private fun peatJniRefNativeInit() = PeatJni.nativeInit()
    @Suppress("unused")
    private fun peatJniRefPeatVersion(): String = PeatJni.peatVersion()
    @Suppress("unused")
    private fun peatJniRefTestJni(): String = PeatJni.testJni()
    @Suppress("unused")
    private fun peatJniRefCreateNode(): Long = PeatJni.createNodeJni("a", "b", "c")
    @Suppress("unused")
    private fun peatJniRefCreateNodeWithConfig(): Long =
        PeatJni.createNodeWithConfigJni("a", "b", "c", false, null)
    @Suppress("unused")
    private fun peatJniRefGetGlobalNodeHandle(): Long = PeatJni.getGlobalNodeHandleJni()
    @Suppress("unused")
    private fun peatJniRefFreeNode(h: Long) = PeatJni.freeNodeJni(h)

    // Peer state
    @Suppress("unused")
    private fun peatJniRefNodeId(h: Long): String = PeatJni.nodeIdJni(h)
    @Suppress("unused")
    private fun peatJniRefPeerCount(h: Long): Int = PeatJni.peerCountJni(h)
    @Suppress("unused")
    private fun peatJniRefConnectedPeers(h: Long): String = PeatJni.connectedPeersJni(h)
    @Suppress("unused")
    private fun peatJniRefEndpointSocketAddr(h: Long): String? = PeatJni.endpointSocketAddrJni(h)
    @Suppress("unused")
    private fun peatJniRefConnectPeer(h: Long, n: String, a: String): Boolean =
        PeatJni.connectPeerJni(h, n, a)

    // Sync coordination
    @Suppress("unused")
    private fun peatJniRefStartSync(h: Long): Boolean = PeatJni.startSyncJni(h)
    @Suppress("unused")
    private fun peatJniRefRequestSync(h: Long): Boolean = PeatJni.requestSyncJni(h)

    // Generic document I/O
    @Suppress("unused")
    private fun peatJniRefPublishDocument(h: Long, c: String, j: String): String =
        PeatJni.publishDocumentJni(h, c, j)
    @Suppress("unused")
    private fun peatJniRefPublishDocumentWithOrigin(h: Long, c: String, j: String, o: String): String =
        PeatJni.publishDocumentWithOriginJni(h, c, j, o)
    @Suppress("unused")
    private fun peatJniRefGetDocument(h: Long, c: String, d: String): String? =
        PeatJni.getDocumentJni(h, c, d)

    // Typed collection accessors
    @Suppress("unused")
    private fun peatJniRefGetCells(h: Long): String = PeatJni.getCellsJni(h)
    @Suppress("unused")
    private fun peatJniRefGetTracks(h: Long): String = PeatJni.getTracksJni(h)
    @Suppress("unused")
    private fun peatJniRefGetPlatforms(h: Long): String = PeatJni.getPlatformsJni(h)
    @Suppress("unused")
    private fun peatJniRefGetCommands(h: Long): String = PeatJni.getCommandsJni(h)
    @Suppress("unused")
    private fun peatJniRefGetMarkers(h: Long): String = PeatJni.getMarkersJni(h)
    @Suppress("unused")
    private fun peatJniRefPublishPlatform(h: Long, j: String): Boolean =
        PeatJni.publishPlatformJni(h, j)
    @Suppress("unused")
    private fun peatJniRefPublishMarker(h: Long, j: String): Boolean =
        PeatJni.publishMarkerJni(h, j)
    @Suppress("unused")
    private fun peatJniRefIngestPosition(h: Long, j: String): String =
        PeatJni.ingestPositionJni(h, j)

    // Blob transfer
    @Suppress("unused")
    private fun peatJniRefEnableBlobTransfer(h: Long, d: String): Boolean =
        PeatJni.enableBlobTransferJni(h, d)
    @Suppress("unused")
    private fun peatJniRefBlobAddPeer(h: Long, p: String, a: String): Boolean =
        PeatJni.blobAddPeerJni(h, p, a)
    @Suppress("unused")
    private fun peatJniRefBlobPut(h: Long, d: ByteArray, c: String): String =
        PeatJni.blobPutJni(h, d, c)
    @Suppress("unused")
    private fun peatJniRefBlobGet(h: Long, hash: String): ByteArray =
        PeatJni.blobGetJni(h, hash)
    @Suppress("unused")
    private fun peatJniRefBlobExistsLocally(h: Long, hash: String): Boolean =
        PeatJni.blobExistsLocallyJni(h, hash)
    @Suppress("unused")
    private fun peatJniRefBlobEndpointId(h: Long): String = PeatJni.blobEndpointIdJni(h)

    // BLE transport state
    @Suppress("unused")
    private fun peatJniRefBleSetStarted(h: Long, s: Boolean) = PeatJni.bleSetStartedJni(h, s)
    @Suppress("unused")
    private fun peatJniRefBleAddPeer(h: Long, p: String) = PeatJni.bleAddPeerJni(h, p)
    @Suppress("unused")
    private fun peatJniRefBleRemovePeer(h: Long, p: String) = PeatJni.bleRemovePeerJni(h, p)
    @Suppress("unused")
    private fun peatJniRefBleIsAvailable(h: Long): Boolean = PeatJni.bleIsAvailableJni(h)
    @Suppress("unused")
    private fun peatJniRefBlePeerCount(h: Long): Int = PeatJni.blePeerCountJni(h)

    // Test-only fault injection
    @Suppress("unused")
    private fun peatJniRefForceStoreError(h: Long): Boolean =
        PeatJni.forceStoreErrorForTestingJni(h)
}
