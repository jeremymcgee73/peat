package com.defenseunicorns.peat

import androidx.annotation.VisibleForTesting

/**
 * Canonical Kotlin bindings for peat-ffi's JNI surface.
 *
 * Before 0.1.2, the AAR shipped only the native `.so` binaries —
 * consumers had to hand-roll matching `external fun` declarations
 * locally. That meant any rename or signature change in peat-ffi's
 * `Java_com_defenseunicorns_peat_PeatJni_*` extern fns surfaced as
 * an `UnsatisfiedLinkError` or `RegisterNatives` SIGABRT at
 * instrumented-test runtime, not at PR-gate time on either side.
 * peat-mesh#145 QA called this out as the binding-drift WARNING.
 *
 * 0.1.2 (peat#886) ships this canonical declaration so all
 * consumers import the same source of truth. Method signatures
 * stay in lockstep with peat-ffi/src/lib.rs by definition — they
 * live in the same release artifact.
 *
 * Note: every method is `@JvmStatic`. peat-ffi's extern fns are
 * declared with `class: JClass` (static-method receiver). Without
 * `@JvmStatic`, members of a Kotlin `object` compile as instance
 * methods on the singleton, and `RegisterNatives` aborts with
 * "jclass has wrong type" SIGABRT at library load.
 *
 * Subscription methods (subscribeDocumentChangesJni,
 * subscribeOutboundFramesJni, and their unsubscribe pairs) are NOT
 * declared here because they take consumer-supplied listener
 * interfaces (`DocumentChangeListener`, `OutboundFrameListener`).
 * Consumers that use them declare those externs locally alongside
 * their listener implementations — same as the pre-0.1.2 pattern
 * for those specific methods.
 */
object PeatJni {
    init {
        System.loadLibrary("peat_ffi")
        // Re-register natives against the live classloader. This
        // matters when Android's classloader isolation prevents
        // JNI_OnLoad from finding PeatJni at .so load time —
        // calling nativeInit() lazily on first access defeats that.
        nativeInit()
    }

    // -- Lifecycle ---------------------------------------------------------

    @JvmStatic external fun nativeInit()

    @JvmStatic external fun peatVersion(): String

    @JvmStatic external fun testJni(): String

    /**
     * Plumb the Android [context] (typically the Application Context)
     * into peat-ffi's `ndk-context` global cell.
     *
     * `JNI_OnLoad` initializes `ndk-context` with the `JavaVM*` it
     * receives but passes `null` for the Context — no Context exists
     * yet at `System.loadLibrary` time. That's enough for the iroh
     * discovery subtree (swarm-discovery / mDNS) which only needs the
     * JVM for thread attachment. It is NOT enough for code that
     * touches the Context itself: `hickory-resolver`'s Android
     * `ConnectivityManager` probe reachable via iroh-dns, NDK asset
     * manager access, etc. Those paths panic with "android context
     * was not initialized" on first call.
     *
     * Consumers using iroh DNS-based discovery (relay, pkarr, non-mDNS
     * peer lookups) **must** call this from `Application.onCreate()`
     * before the first `createNodeJni`. Consumers using only mDNS
     * local-link discovery (QUICKSTART scenarios 1–3, peat-ffi's own
     * surface tests) can skip it.
     *
     * Pass a process-stable reference (the Application instance is the
     * canonical choice — `applicationContext` from any activity).
     * Activity references will be invalidated on configuration change.
     *
     * **Call from `Application.onCreate()`, before any `createNodeJni`.**
     * Multiple calls are tolerated but only safe pre-iroh-start: the
     * Rust implementation releases and reinitializes `ndk-context`
     * under a Mutex, but a concurrent iroh worker reaching
     * `ndk_context::android_context()` during that brief window sees
     * the cell empty and panics.
     *
     * As of peat#924 the Rust side enforces this with an
     * `AtomicBool` flag set on the first successful `createNodeJni`
     * /`createNodeWithConfigJni`: a call here AFTER that point is
     * dropped silently with an `I PeatFFI : setAndroidContextJni:
     * ignoring — iroh already started ...` line in logcat, not a
     * SIGABRT. No exception is thrown — the Kotlin signature
     * returns `Unit` and a misordered call appears as a no-op.
     * Tail logcat with `adb logcat *:I | grep setAndroidContextJni`
     * during development if you suspect the wiring isn't happening.
     * peat#925 QA WARNING follow-up.
     */
    @JvmStatic external fun setAndroidContextJni(context: Any)

    /**
     * Returns true iff `ndk-context`'s stored Android Context is
     * non-null — i.e., a prior [setAndroidContextJni] call has wired
     * a real Application Context through. False before that call
     * (the `null` placeholder JNI_OnLoad installs is the safe default
     * for mDNS-only paths).
     *
     * Surface-tier test hook only (peat#925 QA BLOCKER follow-up).
     * Production code should not consult this — the
     * `setAndroidContextJni`-required-or-not decision is a deployment
     * concern documented on that method, not a runtime check.
     */
    @JvmStatic external fun verifyAndroidContextJni(): Boolean

    @JvmStatic external fun createNodeJni(
        appId: String,
        sharedKey: String,
        storagePath: String,
    ): Long

    @JvmStatic external fun createNodeWithConfigJni(
        appId: String,
        sharedKey: String,
        storagePath: String,
        enableBle: Boolean,
        blePowerProfile: String?,
    ): Long

    @JvmStatic external fun getGlobalNodeHandleJni(): Long

    @JvmStatic external fun freeNodeJni(handle: Long)

    // -- Node identity / peer state ----------------------------------------

    @JvmStatic external fun nodeIdJni(handle: Long): String

    @JvmStatic external fun peerCountJni(handle: Long): Int

    @JvmStatic external fun connectedPeersJni(handle: Long): String

    /**
     * Returns this node's bound iroh socket address as `"ip:port"`,
     * or null if no socket has been bound. Used as the `address`
     * argument to [connectPeerJni] when two in-process instances
     * need to dial each other without a discovery layer.
     */
    @JvmStatic external fun endpointSocketAddrJni(handle: Long): String?

    /**
     * Atomically populates the iroh address lookup with the peer's
     * `(nodeId, address)` mapping and dials. Returns true on success.
     */
    @JvmStatic external fun connectPeerJni(
        handle: Long,
        nodeId: String,
        address: String,
    ): Boolean

    // -- Sync coordination -------------------------------------------------

    /**
     * Starts the sync coordination layer (accept loop + observer
     * forwarder). MUST be called after `createNodeJni`, before any
     * publish/sync calls — otherwise the receive side never
     * processes inbound sync messages.
     */
    @JvmStatic external fun startSyncJni(handle: Long): Boolean

    /**
     * Iterates every connected peer and runs
     * `sync_all_documents_with_peer` on each. Fire-and-forget;
     * returns true on submission, not on convergence.
     */
    @JvmStatic external fun requestSyncJni(handle: Long): Boolean

    // -- Generic document I/O ----------------------------------------------

    /**
     * Publishes a JSON document. The `id` field is hoisted to
     * Document::id during ingestion. Returns the assigned id.
     */
    @JvmStatic external fun publishDocumentJni(
        handle: Long,
        collection: String,
        json: String,
    ): String

    /**
     * Origin-aware variant. The `origin` argument identifies a
     * transport in cross-transport fan-out so the document doesn't
     * loop back out on the same transport it came in on
     * (ADR-059 Amendment 2).
     */
    @JvmStatic external fun publishDocumentWithOriginJni(
        handle: Long,
        collection: String,
        json: String,
        origin: String,
    ): String

    /**
     * Reads a document back as JSON. Returns null if the document
     * doesn't exist locally. Throws `RuntimeException` if the
     * underlying store read fails (distinguishes "not yet synced"
     * from "storage broken").
     */
    @JvmStatic external fun getDocumentJni(
        handle: Long,
        collection: String,
        docId: String,
    ): String?

    // -- Typed collection accessors (CoT-style schema; ADR-049) ------------

    @JvmStatic external fun getCellsJni(handle: Long): String

    @JvmStatic external fun getTracksJni(handle: Long): String

    @JvmStatic external fun getNodesJni(handle: Long): String

    @JvmStatic external fun getCommandsJni(handle: Long): String

    @JvmStatic external fun getMarkersJni(handle: Long): String

    @JvmStatic external fun publishNodeJni(handle: Long, nodeJson: String): Boolean

    @JvmStatic external fun publishMarkerJni(handle: Long, markerJson: String): Boolean

    @JvmStatic external fun ingestPositionJni(handle: Long, positionJson: String): String

    // -- Blob transfer -----------------------------------------------------

    @JvmStatic external fun enableBlobTransferJni(handle: Long, blobDir: String): Boolean

    @JvmStatic external fun blobAddPeerJni(
        handle: Long,
        peerId: String,
        address: String,
    ): Boolean

    @JvmStatic external fun blobPutJni(
        handle: Long,
        data: ByteArray,
        contentType: String,
    ): String

    @JvmStatic external fun blobGetJni(handle: Long, hash: String): ByteArray

    @JvmStatic external fun blobExistsLocallyJni(handle: Long, hash: String): Boolean

    @JvmStatic external fun blobEndpointIdJni(handle: Long): String

    // -- BLE transport state (ADR-039) -------------------------------------

    @JvmStatic external fun bleSetStartedJni(handle: Long, started: Boolean)

    @JvmStatic external fun bleAddPeerJni(handle: Long, peerId: String)

    @JvmStatic external fun bleRemovePeerJni(handle: Long, peerId: String)

    @JvmStatic external fun bleIsAvailableJni(handle: Long): Boolean

    @JvmStatic external fun blePeerCountJni(handle: Long): Int

    // -- Test-only fault injection (peat#885; present in production
    //    .so builds, gated by the "ForTesting" naming convention rather
    //    than cfg(test). Calling it from production code does no harm
    //    beyond setting a one-shot flag that the next getDocumentJni
    //    consumes via swap(false, ...) and produces a fail-fast
    //    RuntimeException for. Production callers never arm it.) ----

    /**
     * Arms a one-shot fault injection: the next [getDocumentJni]
     * call returns Err on the Rust side, which propagates as
     * `RuntimeException` on the Kotlin side. Self-clears after one
     * trigger. Test-only; do not call in production code.
     *
     * Returns true if the flag was successfully armed.
     *
     * `@VisibleForTesting(otherwise = NONE)` makes lint flag every
     * non-test caller as an error. The Rust side's naming-convention
     * guard prevents harm if a production call slips through, but
     * this gives IDE + lint tooling a PR-gate signal too.
     */
    @JvmStatic
    @VisibleForTesting(otherwise = VisibleForTesting.NONE)
    external fun forceStoreErrorForTestingJni(handle: Long): Boolean
}
