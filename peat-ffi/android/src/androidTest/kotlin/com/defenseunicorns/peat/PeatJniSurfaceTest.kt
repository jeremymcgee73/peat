package com.defenseunicorns.peat

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.FixMethodOrder
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.runners.MethodSorters

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
@FixMethodOrder(MethodSorters.NAME_ASCENDING)
// NAME_ASCENDING ordering is load-bearing for the `a_*` /
// `z_*` test pair below — `setAndroidContextJni`'s
// happy-path round-trip must run before any `createNodeJni`
// call sets the process-wide `IROH_STARTED` flag (peat#924
// QA WARNING-2 runtime guard); the rejection-path test runs
// after, so the `z_` prefix sorts it last.
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
        try {
            PeatJni.unsubscribeDocumentChangesJni()
        } catch (t: Throwable) {
            // Best-effort cleanup; don't mask test failures.
        }
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
        val handle = PeatJni.createNodeJni(APP_ID, SHARED_KEY, storageDir.absolutePath, "127.0.0.1")
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
     * peat#925 QA BLOCKER: instrumented surface-tier coverage for
     * the `setAndroidContextJni` plumbing introduced in this PR.
     *
     * `JNI_OnLoad` initializes `ndk-context` with a `null` Context
     * placeholder (no Application exists yet at `System.loadLibrary`
     * time). Consumers that exercise iroh DNS-based discovery (relay,
     * pkarr, non-mDNS peer lookups) — which transitively touch
     * `hickory-resolver`'s Android ConnectivityManager probe — must
     * promote the real Application Context into the global cell via
     * `setAndroidContextJni` from `Application.onCreate`. Without
     * the wiring, those code paths panic with "android context was
     * not initialized" and SIGABRT the process.
     *
     * This test exercises the full Kotlin → JNI → Rust →
     * `ndk_context` round-trip:
     *
     *  1. `verifyAndroidContextJni()` returns false (JNI_OnLoad
     *     installed the null placeholder).
     *  2. `setAndroidContextJni(targetContext)` promotes the
     *     instrumentation's Context jobject via `NewGlobalRef`,
     *     stores it in `ANDROID_CONTEXT_GLOBAL_REF`, and re-inits
     *     `ndk_context` with that GlobalRef's jobject handle.
     *  3. `verifyAndroidContextJni()` now returns true.
     *
     * Returning true is end-to-end evidence that the Kotlin Any
     * argument marshalled correctly across JNI, the GlobalRef
     * promotion succeeded, the raw-pointer hand-off to
     * `ndk_context::initialize_android_context` is intact, and the
     * `ndk_context::android_context().context()` accessor returns
     * the non-null pointer downstream Android-aware crates need.
     *
     * A regression in any link of that chain — broken `new_global_ref`
     * call, wrong JNI sig in the NativeMethod table, lost GlobalRef
     * reaching the global cell — surfaces as
     * `verifyAndroidContextJni()` returning false here, not as a
     * mysterious downstream SIGABRT in production deployments.
     */
    /**
     * peat#925 QA BLOCKER + peat#924 QA WARNING-2: the happy path
     * for the `setAndroidContextJni` plumbing. This test exercises
     * the full Kotlin → JNI → Rust → `ndk_context` round-trip:
     * Kotlin `Any` → JNI jobject local ref → `NewGlobalRef` →
     * `release_android_context()` + `initialize_android_context(vm,
     * ctx)` → `ndk_context::android_context().context()` returns
     * the GlobalRef'd jobject (non-null).
     *
     * **Method name prefix `a_` is load-bearing.** `NAME_ASCENDING`
     * ordering on the class puts this test first, before any
     * `createNodeJni` call sets the process-wide `IROH_STARTED`
     * atomic flag. After that flag is set, `setAndroidContextJni`
     * returns early (logged as "ignoring — iroh already started"
     * in logcat) — the rejection path is exercised by the `z_`
     * test below.
     */
    @Test
    fun a_setAndroidContextJni_wiresContextThroughToNdkContext() {
        val targetContext = InstrumentationRegistry.getInstrumentation().targetContext
        assertNotNull("instrumentation must provide a target Context", targetContext)

        PeatJni.setAndroidContextJni(targetContext)

        assertTrue(
            "after setAndroidContextJni, ndk_context().context() must be non-null — confirms " +
                "Kotlin Any → JNI jobject → NewGlobalRef → ndk_context::initialize_android_context " +
                "round-trip is intact",
            PeatJni.verifyAndroidContextJni(),
        )
    }

    /**
     * Pins a mobile-app consumer entrypoint: explicit logical identity plus
     * a concrete Wi-Fi interface address. Successful creation proves the
     * seven-argument Kotlin descriptor matches the registered Rust JNI method;
     * the endpoint assertion proves `bindAddress` reaches the Iroh bind.
     */
    @Test
    fun b_createNodeWithConfigJni_acceptsIdentityAndConcreteBindAddress() {
        val cacheDir =
            InstrumentationRegistry.getInstrumentation()
                .targetContext
                .cacheDir
        val storageDir =
            File(cacheDir, "peat-ffi-surface-config-${System.nanoTime()}").apply {
                mkdirs()
            }
        storageDirs.add(storageDir)

        val handle =
            PeatJni.createNodeWithConfigJni(
                APP_ID,
                SHARED_KEY,
                "android-surface-config",
                storageDir.absolutePath,
                false,
                null,
                "127.0.0.1",
            )
        assertTrue(
            "createNodeWithConfigJni returned 0 for the seven-argument consumer path",
            handle != 0L,
        )
        handles.add(handle)

        val endpointAddress = PeatJni.endpointSocketAddrJni(handle)
        assertNotNull("configured node must expose its bound endpoint", endpointAddress)
        assertTrue(
            "bindAddress must reach the Iroh endpoint; got $endpointAddress",
            endpointAddress!!.startsWith("127.0.0.1:"),
        )
        assertTrue(
            "explicit nodeId must produce a non-empty formation endpoint identity",
            PeatJni.nodeIdJni(handle).isNotEmpty(),
        )
    }

    /**
     * peat#924 QA WARNING-2 round 2: the `IROH_STARTED` runtime
     * guard rejects `setAndroidContextJni` calls after the first
     * successful `createNodeJni`. By the time this test runs
     * (NAME_ASCENDING orders it last, after `forceStoreError*` and
     * `loadLibrarySucceeds*` and the `a_*` happy-path test),
     * `IROH_STARTED` is true. Calling `setAndroidContextJni` again
     * must be a no-op: the call returns without panicking, no
     * `ndk_context::release_android_context() → initialize_android_context()`
     * window opens, and `verifyAndroidContextJni()` continues to
     * report whatever the `a_*` test installed — neither cleared
     * nor replaced.
     *
     * The point of this test is the *non-abort* invariant: a
     * consumer that ignores the Kotlin KDoc and calls
     * `setAndroidContextJni` from a post-`createNodeJni` callback
     * gets a logged no-op, not a SIGABRT.
     *
     * Prefix `z_` is load-bearing for ordering (see class-level
     * `@FixMethodOrder` comment).
     */
    @Test
    fun z_setAndroidContextJni_isNoOpAfterIrohStart() {
        // Pre-condition: some prior test in this class has called
        // createNode (forceStoreErrorForTesting_throwsOnce_thenClears
        // and forceStoreErrorForTesting_invalidHandle_doesNotArm
        // both do, and they sort before this one under
        // NAME_ASCENDING). That set IROH_STARTED. We don't assert
        // that pre-condition directly — there's no JNI verb for
        // reading IROH_STARTED — but the assertion below holds iff
        // the guard fires.
        val targetContext = InstrumentationRegistry.getInstrumentation().targetContext
        assertNotNull("instrumentation must provide a target Context", targetContext)

        // The `a_*` test above installed a non-null Context. Snapshot
        // the state so we can verify the no-op doesn't disturb it.
        val priorState = PeatJni.verifyAndroidContextJni()
        assertTrue(
            "precondition for this test: a_setAndroidContextJni_* must have run first " +
                "and installed the Context — if this fails, NAME_ASCENDING ordering broke",
            priorState,
        )

        // The actual rejection invocation. Must not throw, must not
        // SIGABRT, must not clear the prior Context.
        PeatJni.setAndroidContextJni(targetContext)

        assertTrue(
            "after setAndroidContextJni was called post-iroh-start, the prior Context must " +
                "still be installed — confirming the AtomicBool guard short-circuited the " +
                "release+reinit window rather than opening it (peat#924 QA WARNING-2 guard)",
            PeatJni.verifyAndroidContextJni(),
        )
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
        val collection = "nodes"
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
        val result = PeatJni.getDocumentJni(handle, "nodes", "anything")
        assertNull(
            "getDocumentJni must not be in armed state after a 0-handle arm attempt",
            result,
        )
    }

    /**
     * peat#978 / peat#1082 surface gate: the JNI entry points added for the
     * BLE doc-sync fix — `clearGlobalNodeHandleJni`, `ingestInboundFrameJni`,
     * `ingestInboundLiteFrameJni`, and the outbound-frame subscription — must
     * be REGISTERED (RegisterNatives) so
     * the AAR's matching `external fun` declarations resolve. A missing
     * registration surfaces here as `UnsatisfiedLinkError` at AAR-test time,
     * not as a downstream consumer link failure.
     *
     * Exercises the JNI marshaling + the handle-0 guard without a peer:
     *  - `clearGlobalNodeHandleJni()` is a safe no-op (idempotent teardown).
     *  - `ingestInbound{,Lite}FrameJni(0, ..)` returns null via the handle-0
     *    guard — confirming String/ByteArray marshalling + the early return
     *    (a null `Arc::from_raw(0)` would otherwise be UB).
     *  - `subscribeOutboundFramesJni(0, ..)` returns false and unsubscribe is
     *    an idempotent no-op, proving the canonical listener descriptor links.
     */
    @Test
    fun bleIngestAndClearGlobalHandle_registered_andGuardHandleZero() {
        // No-op clear must link + not throw (no node required).
        PeatJni.clearGlobalNodeHandleJni()

        val frame = byteArrayOf(0xB6.toByte(), 0x01, 0x02, 0x03)
        assertNull(
            "ingestInboundFrameJni(handle=0) must return null via the handle-0 guard",
            PeatJni.ingestInboundFrameJni(0L, "tracks", frame),
        )
        assertNull(
            "ingestInboundLiteFrameJni(handle=0) must return null via the handle-0 guard",
            PeatJni.ingestInboundLiteFrameJni(0L, "demo", frame),
        )

        val listener = OutboundFrameListener { _, _, _ ->
            throw AssertionError("handle-0 subscription must never invoke the listener")
        }
        assertEquals(
            "subscribeOutboundFramesJni(handle=0) must return false",
            false,
            PeatJni.subscribeOutboundFramesJni(0L, listener),
        )
        PeatJni.unsubscribeOutboundFramesJni(0L)
    }

    /**
     * Locks the direct-JNI document notification contract into the AAR. This
     * proves both RegisterNatives descriptors resolve and that a committed
     * local document reaches the packaged listener on Rust's runtime thread.
     */
    @Test
    fun documentChangeSubscription_registered_andDeliversCommittedKey() {
        val invalidListener =
            object : DocumentChangeListener {
                override fun onChange(collection: String, docId: String) {
                    fail("handle-0 subscription must not deliver document changes")
                }

                override fun onError(message: String) {
                    fail("handle-0 subscription must not deliver errors: $message")
                }
            }
        assertEquals(
            "subscribeDocumentChangesJni(handle=0) must return false",
            false,
            PeatJni.subscribeDocumentChangesJni(0L, invalidListener),
        )
        PeatJni.unsubscribeDocumentChangesJni()

        val handle = createNode("document-change")
        assertTrue("startSyncJni must succeed", PeatJni.startSyncJni(handle))
        val delivered = CountDownLatch(1)
        val receivedKey = AtomicReference<Pair<String, String>>()
        val callbackError = AtomicReference<String>()
        val listener =
            object : DocumentChangeListener {
                override fun onChange(collection: String, docId: String) {
                    receivedKey.set(collection to docId)
                    delivered.countDown()
                }

                override fun onError(message: String) {
                    callbackError.set(message)
                    delivered.countDown()
                }
            }
        assertTrue(
            "valid node must accept document-change subscription",
            PeatJni.subscribeDocumentChangesJni(handle, listener),
        )

        val collection = "surface-documents"
        val documentId = "callback-proof"
        assertEquals(
            documentId,
            PeatJni.publishDocumentJni(
                handle,
                collection,
                """{"id":"$documentId","value":"proof"}""",
            ),
        )
        assertTrue(
            "document-change callback was not delivered within 5 seconds; error=${callbackError.get()}",
            delivered.await(5, TimeUnit.SECONDS),
        )
        assertNull("document-change callback reported an error", callbackError.get())
        assertEquals(collection to documentId, receivedKey.get())
        PeatJni.unsubscribeDocumentChangesJni()
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
     * Both document-change and outbound-frame subscriptions are canonical and
     * therefore appear here.
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
            ::peatJniRefClearGlobalNodeHandle,
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
            ::peatJniRefSubscribeDocumentChanges,
            ::peatJniRefUnsubscribeDocumentChanges,
            // Typed collection accessors
            ::peatJniRefGetCells,
            ::peatJniRefGetTracks,
            ::peatJniRefGetNodes,
            ::peatJniRefGetCommands,
            ::peatJniRefGetMarkers,
            ::peatJniRefPublishNode,
            ::peatJniRefPublishMarker,
            ::peatJniRefIngestPosition,
            ::peatJniRefIngestInboundFrame,
            ::peatJniRefIngestInboundLiteFrame,
            ::peatJniRefSubscribeOutboundFrames,
            ::peatJniRefUnsubscribeOutboundFrames,
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
        // 44 PeatJni methods total. If this number changes, the
        // count below must change too — and the new method needs
        // its own peatJniRef* shim added above.
        assertEquals(44, refs.size)
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
    private fun peatJniRefCreateNode(): Long = PeatJni.createNodeJni("a", "b", "c", null)
    @Suppress("unused")
    private fun peatJniRefCreateNodeWithConfig(): Long =
        PeatJni.createNodeWithConfigJni("a", "b", null, "c", false, null, null)
    @Suppress("unused")
    private fun peatJniRefGetGlobalNodeHandle(): Long = PeatJni.getGlobalNodeHandleJni()
    @Suppress("unused")
    private fun peatJniRefClearGlobalNodeHandle() = PeatJni.clearGlobalNodeHandleJni()
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
    @Suppress("unused")
    private fun peatJniRefSubscribeDocumentChanges(h: Long, l: DocumentChangeListener): Boolean =
        PeatJni.subscribeDocumentChangesJni(h, l)
    @Suppress("unused")
    private fun peatJniRefUnsubscribeDocumentChanges() =
        PeatJni.unsubscribeDocumentChangesJni()

    // Typed collection accessors
    @Suppress("unused")
    private fun peatJniRefGetCells(h: Long): String = PeatJni.getCellsJni(h)
    @Suppress("unused")
    private fun peatJniRefGetTracks(h: Long): String = PeatJni.getTracksJni(h)
    @Suppress("unused")
    private fun peatJniRefGetNodes(h: Long): String = PeatJni.getNodesJni(h)
    @Suppress("unused")
    private fun peatJniRefGetCommands(h: Long): String = PeatJni.getCommandsJni(h)
    @Suppress("unused")
    private fun peatJniRefGetMarkers(h: Long): String = PeatJni.getMarkersJni(h)
    @Suppress("unused")
    private fun peatJniRefPublishNode(h: Long, j: String): Boolean =
        PeatJni.publishNodeJni(h, j)
    @Suppress("unused")
    private fun peatJniRefPublishMarker(h: Long, j: String): Boolean =
        PeatJni.publishMarkerJni(h, j)
    @Suppress("unused")
    private fun peatJniRefIngestPosition(h: Long, j: String): String =
        PeatJni.ingestPositionJni(h, j)
    @Suppress("unused")
    private fun peatJniRefIngestInboundFrame(h: Long, c: String, b: ByteArray): String? =
        PeatJni.ingestInboundFrameJni(h, c, b)
    @Suppress("unused")
    private fun peatJniRefIngestInboundLiteFrame(h: Long, c: String, b: ByteArray): String? =
        PeatJni.ingestInboundLiteFrameJni(h, c, b)
    @Suppress("unused")
    private fun peatJniRefSubscribeOutboundFrames(h: Long, l: OutboundFrameListener): Boolean =
        PeatJni.subscribeOutboundFramesJni(h, l)
    @Suppress("unused")
    private fun peatJniRefUnsubscribeOutboundFrames(h: Long) =
        PeatJni.unsubscribeOutboundFramesJni(h)

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
