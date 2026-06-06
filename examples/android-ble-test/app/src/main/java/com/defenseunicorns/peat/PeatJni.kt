/*
 * Copyright (c) 2026 Defense Unicorns.  All rights reserved.
 */

package com.defenseunicorns.peat

import android.util.Log

/**
 * Direct JNI bindings for Peat FFI.
 *
 * This bypasses JNA/UniFFI which has symbol lookup issues on Android
 * due to linker namespace isolation. Uses standard JNI with native
 * method declarations that map directly to Rust #[no_mangle] exports.
 *
 * IMPORTANT: Call initNatives() after System.load() to register the native methods.
 * This is required because Android's classloader namespace isolation prevents
 * automatic JNI symbol lookup.
 */
object PeatJni {
    private const val TAG = "PeatJni"
    private var initialized = false

    /**
     * Initialize native methods by registering them via JNI RegisterNatives.
     * This MUST be called after System.load() and before any other native methods.
     *
     * @return true if initialization succeeded
     */
    fun initNatives(): Boolean {
        if (initialized) {
            Log.d(TAG, "Already initialized")
            return true
        }

        return try {
            Log.d(TAG, "Calling nativeInit to register native methods...")
            nativeInit()
            initialized = true
            Log.i(TAG, "Native methods registered successfully")
            true
        } catch (e: UnsatisfiedLinkError) {
            Log.e(TAG, "nativeInit failed - UnsatisfiedLinkError: ${e.message}")
            false
        } catch (e: Exception) {
            Log.e(TAG, "nativeInit failed - Exception: ${e.message}")
            false
        }
    }

    /**
     * Native initialization function - registers all other native methods.
     * This is the ONLY function that can be found via standard JNI lookup
     * after System.load() due to Android namespace isolation.
     */
    @JvmStatic
    private external fun nativeInit()

    /**
     * Get Peat library version string.
     */
    @JvmStatic
    external fun peatVersion(): String

    /**
     * Test that JNI bindings work.
     */
    @JvmStatic
    external fun testJni(): String

    /**
     * Create a Peat node and return its handle.
     * @param appId Formation/app identifier
     * @param sharedKey Base64-encoded shared key
     * @param storagePath Path for persistent storage
     * @return Handle (pointer) to the PeatNode, or 0 on failure
     */
    @JvmStatic
    external fun createNodeJni(appId: String, sharedKey: String, storagePath: String): Long

    /**
     * Create a Peat node with transport configuration (ADR-039, #558).
     *
     * This extended version supports BLE transport configuration for unified
     * multi-transport operation. When enableBle is true, the node will attempt
     * to initialize BLE transport alongside the default Iroh transport.
     *
     * Note: Full BLE support on Android requires the Android BLE adapter integration
     * in peat-btle. Currently, BLE transport is deferred on Android until the
     * adapter callbacks are implemented.
     *
     * @param appId Formation/app identifier
     * @param sharedKey Base64-encoded shared key
     * @param storagePath Path for persistent storage
     * @param enableBle Whether to enable BLE transport
     * @param blePowerProfile BLE power profile: "aggressive", "balanced", or "low_power" (null for default)
     * @return Handle (pointer) to the PeatNode, or 0 on failure
     */
    @JvmStatic
    external fun createNodeWithConfigJni(
        appId: String,
        sharedKey: String,
        storagePath: String,
        enableBle: Boolean,
        blePowerProfile: String?
    ): Long

    /**
     * Get the node ID (hex-encoded public key) for a node handle.
     * @param handle Node handle from createNodeJni
     * @return Node ID string, or error message
     */
    @JvmStatic
    external fun nodeIdJni(handle: Long): String

    /**
     * Get the current peer count for a node.
     * @param handle Node handle from createNodeJni
     * @return Number of connected peers, or -1 on error
     */
    @JvmStatic
    external fun peerCountJni(handle: Long): Int

    /**
     * Get connected peer IDs as a JSON array.
     * @param handle Node handle from createNodeJni
     * @return JSON array of hex-encoded peer IDs, e.g. ["abc123...", "def456..."]
     */
    @JvmStatic
    external fun connectedPeersJni(handle: Long): String

    /**
     * Start sync for a node.
     * @param handle Node handle from createNodeJni
     * @return true if sync started successfully
     */
    @JvmStatic
    external fun startSyncJni(handle: Long): Boolean

    /**
     * Free a PeatNode handle.
     * Must be called when done with a node to avoid memory leaks.
     * @param handle Node handle from createNodeJni
     */
    @JvmStatic
    external fun freeNodeJni(handle: Long)

    /**
     * Get the global node handle that survives APK replacement.
     * @return Handle (pointer) to the PeatNode, or 0 if no node exists
     */
    @JvmStatic
    external fun getGlobalNodeHandleJni(): Long

    /**
     * Get all cells as JSON array string.
     * @param handle Node handle from createNodeJni
     * @return JSON array of cell objects, or "[]" on error
     */
    @JvmStatic
    external fun getCellsJni(handle: Long): String

    /**
     * Get all tracks as JSON array string.
     * @param handle Node handle from createNodeJni
     * @return JSON array of track objects, or "[]" on error
     */
    @JvmStatic
    external fun getTracksJni(handle: Long): String

    /**
     * Get all nodes as JSON array string.
     * @param handle Node handle from createNodeJni
     * @return JSON array of node objects, or "[]" on error
     */
    @JvmStatic
    external fun getNodesJni(handle: Long): String

    /**
     * Publish a node (self-position/PLI) to the Peat network.
     * @param handle Node handle from createNodeJni
     * @param nodeJson JSON string representing the node data
     * @return true if published successfully
     */
    @JvmStatic
    external fun publishNodeJni(handle: Long, nodeJson: String): Boolean

    /**
     * Publish a generic document to a named collection on this node.
     *
     * Backed by `peat_mesh::Node::publish` — works for any collection
     * (chats, markers, alerts, custom doc types). Use the typed
     * `publish<Type>Jni` methods (e.g. `publishNodeJni`) when they
     * exist; this is the generic escape hatch.
     *
     * The JSON must be an object. Top-level keys become the document
     * body. The `"id"` field is **optional**:
     * - If present and a string, it becomes the document's id.
     * - If absent, or present but not a string, the backend assigns an
     *   id (UUID under the in-memory backend; backend-defined elsewhere).
     *
     * The returned String is the document id that was actually used —
     * either the one the caller supplied or the backend-assigned one.
     * Callers needing a stable id MUST capture the return value rather
     * than assuming the JSON's `"id"` won.
     *
     * @param handle Node handle from createNodeJni.
     * @param collection Collection name (e.g. "chats", "markers").
     * @param json JSON object as a string. `"id"` is optional.
     * @return The document id on success (caller-supplied or
     *         backend-assigned), or empty string on failure (handle
     *         invalid, JSON malformed, JSON not an object, publish error).
     */
    @JvmStatic
    external fun publishDocumentJni(handle: Long, collection: String, json: String): String

    /**
     * Ingest a peat-btle BLE position from a peer through the cross-transport
     * BLE gateway. Translated to a track document via `BleTranslator` and
     * published into the underlying mesh `Node`; iroh-bound peers receive
     * it through Automerge sync. This is the BLE → Node → iroh half of the
     * cross-transport bridge.
     *
     * The `json` envelope mirrors `peat_btle::Position` plus surrounding
     * ingest metadata:
     * ```json
     * {
     *   "lat": 40.7,
     *   "lon": -74.0,
     *   "altitude": 100.0,        // optional
     *   "accuracy": 5.0,          // optional
     *   "peripheral_id": 3405643777,
     *   "callsign": "SCOUT-CAFE", // optional
     *   "mesh_id": "29C916FA"     // optional
     * }
     * ```
     *
     * Required fields: `lat`, `lon`, `peripheral_id`.
     *
     * `peripheral_id` accepts the full u32 range expressed either as a
     * non-negative integer (Kotlin `Long` or `UInt.toLong()`) or as a
     * sign-extended negative integer (Kotlin `Int.toLong()` of a value with
     * the high bit set — e.g. `0xCAFE_0001` reads as `-889323519`). Both
     * round-trip to the same internal u32; values outside
     * `[Int.MIN_VALUE, UInt.MAX_VALUE.toLong()]` are rejected rather than
     * silently truncated. Prefer `Long` or `UInt` at the call site to avoid
     * the sign-extension question entirely.
     *
     * @param handle Node handle from createNodeJni.
     * @param json BLE position envelope as a JSON string.
     * @return Track document id on success (the translator's
     *         `ble-<UPPERCASE-8-HEX>` form), or empty string on failure
     *         (handle invalid, JSON malformed, missing required field,
     *         peripheral_id out of range, publish error, or peat-ffi not
     *         built with the bluetooth feature).
     */
    @JvmStatic
    external fun ingestPositionJni(handle: Long, json: String): String

    /**
     * Connect to a known peer by node ID and socket address (bypasses mDNS).
     * @param handle Node handle from createNodeJni
     * @param nodeId Hex-encoded Iroh node ID of the peer
     * @param address Socket address of the peer (e.g. "192.168.1.100:42009")
     * @return true if connection initiated successfully
     */
    @JvmStatic
    external fun connectPeerJni(handle: Long, nodeId: String, address: String): Boolean

    // ========================================================================
    // BLE Transport JNI Methods (ADR-047 Android Bootstrap)
    // ========================================================================

    /**
     * Signal BLE transport started/stopped.
     * Makes is_available() return true/false for PACE routing.
     * @param handle Node handle from createNodeJni
     * @param started true to start, false to stop
     */
    @JvmStatic
    external fun bleSetStartedJni(handle: Long, started: Boolean)

    /**
     * Add a reachable BLE peer.
     * Makes can_reach(peer) return true for PACE routing.
     * @param handle Node handle from createNodeJni
     * @param peerId Peer ID as 8-char hex string (e.g. "0A1B2C3D")
     */
    @JvmStatic
    external fun bleAddPeerJni(handle: Long, peerId: String)

    /**
     * Remove a reachable BLE peer.
     * Makes can_reach(peer) return false for PACE routing.
     * @param handle Node handle from createNodeJni
     * @param peerId Peer ID as 8-char hex string (e.g. "0A1B2C3D")
     */
    @JvmStatic
    external fun bleRemovePeerJni(handle: Long, peerId: String)

    /**
     * Query whether BLE transport is available (started).
     * @param handle Node handle from createNodeJni
     * @return true if BLE transport has been started
     */
    @JvmStatic
    external fun bleIsAvailableJni(handle: Long): Boolean

    /**
     * Get the number of reachable BLE peers.
     * @param handle Node handle from createNodeJni
     * @return Number of BLE peers added via bleAddPeerJni
     */
    @JvmStatic
    external fun blePeerCountJni(handle: Long): Int

    /**
     * Test if JNI bindings are working.
     * @return true if JNI is functional
     */
    fun test(): Boolean {
        if (!initialized) {
            Log.e(TAG, "JNI test failed - not initialized. Call initNatives() first.")
            return false
        }
        return try {
            val version = peatVersion()
            val testMsg = testJni()
            Log.i(TAG, "JNI test passed - Version: $version, Message: $testMsg")
            true
        } catch (e: UnsatisfiedLinkError) {
            Log.e(TAG, "JNI test failed - UnsatisfiedLinkError: ${e.message}")
            false
        } catch (e: Exception) {
            Log.e(TAG, "JNI test failed - Exception: ${e.message}")
            false
        }
    }
}

/**
 * Wrapper class for a Peat node using JNI.
 * Provides a more idiomatic Kotlin API over the raw JNI functions.
 *
 * Uses a global singleton handle that survives APK replacement to avoid
 * losing the native node connection when the plugin is hot-swapped.
 */
class PeatNodeJni private constructor(private val handle: Long) : AutoCloseable {

    companion object {
        private const val TAG = "PeatNodeJni"

        // Global handle that survives APK replacement
        // The native node lives in native memory which persists across plugin reloads
        @Volatile
        private var globalHandle: Long = 0L

        @Volatile
        private var globalInstance: PeatNodeJni? = null

        /**
         * Create a new Peat node, or return existing one if handle is still valid.
         * @param appId Formation/app identifier
         * @param sharedKey Base64-encoded shared key
         * @param storagePath Path for persistent storage
         * @return PeatNodeJni instance, or null on failure
         */
        fun create(appId: String, sharedKey: String, storagePath: String): PeatNodeJni? =
            createWithConfig(appId, sharedKey, storagePath, enableBle = false, blePowerProfile = null)

        /**
         * Create a new Peat node with transport configuration (ADR-039, #558).
         *
         * This is the preferred method for creating nodes with BLE transport support.
         * When enableBle is true, the node will be configured for unified multi-transport
         * operation, though full BLE support requires Android adapter integration.
         *
         * @param appId Formation/app identifier
         * @param sharedKey Base64-encoded shared key
         * @param storagePath Path for persistent storage
         * @param enableBle Whether to enable BLE transport (default: false)
         * @param blePowerProfile BLE power profile: "aggressive", "balanced", or "low_power"
         * @return PeatNodeJni instance, or null on failure
         */
        fun createWithConfig(
            appId: String,
            sharedKey: String,
            storagePath: String,
            enableBle: Boolean = false,
            blePowerProfile: String? = null
        ): PeatNodeJni? {
            // Check if we have an existing valid handle
            if (globalHandle != 0L) {
                try {
                    // Verify handle is still valid by calling peerCount
                    val peerCount = PeatJni.peerCountJni(globalHandle)
                    if (peerCount >= 0) {
                        Log.i(TAG, "Reusing existing Peat node handle: $globalHandle (peers: $peerCount)")
                        if (globalInstance == null) {
                            globalInstance = PeatNodeJni(globalHandle)
                        }
                        return globalInstance
                    }
                } catch (e: Exception) {
                    Log.w(TAG, "Existing handle invalid, will create new node: ${e.message}")
                    globalHandle = 0L
                    globalInstance = null
                }
            }

            return try {
                val handle = PeatJni.createNodeWithConfigJni(
                    appId,
                    sharedKey,
                    storagePath,
                    enableBle,
                    blePowerProfile
                )
                if (handle != 0L) {
                    Log.i(TAG, "Created Peat node with handle: $handle (BLE: $enableBle)")
                    globalHandle = handle
                    globalInstance = PeatNodeJni(handle)
                    globalInstance
                } else {
                    Log.e(TAG, "Failed to create Peat node (handle=0)")
                    null
                }
            } catch (e: Exception) {
                Log.e(TAG, "Exception creating Peat node: ${e.message}", e)
                null
            }
        }

        /**
         * Get the existing instance without creating a new one.
         * Recovers from native global handle if Kotlin state was lost (APK replacement).
         */
        fun getInstance(): PeatNodeJni? {
            // First check if we have a local instance
            if (globalInstance != null) {
                return globalInstance
            }

            // Try to recover from native global handle (survives APK replacement)
            try {
                val nativeHandle = PeatJni.getGlobalNodeHandleJni()
                if (nativeHandle != 0L) {
                    // Verify handle is still valid
                    val peerCount = PeatJni.peerCountJni(nativeHandle)
                    if (peerCount >= 0) {
                        Log.i(TAG, "Recovered Peat node from native global handle: $nativeHandle (peers: $peerCount)")
                        globalHandle = nativeHandle
                        globalInstance = PeatNodeJni(nativeHandle)
                        return globalInstance
                    }
                }
            } catch (e: Exception) {
                Log.w(TAG, "Failed to recover from native handle: ${e.message}")
            }

            return null
        }
    }

    /**
     * Get this node's ID (hex-encoded public key).
     */
    fun nodeId(): String = PeatJni.nodeIdJni(handle)

    /**
     * Get the current number of connected peers.
     */
    fun peerCount(): Int = PeatJni.peerCountJni(handle)

    /**
     * Get connected peer IDs as a JSON array string.
     * @return JSON array of hex-encoded peer IDs
     */
    fun connectedPeers(): String = PeatJni.connectedPeersJni(handle)

    /**
     * Start P2P sync.
     * @return true if sync started successfully
     */
    fun startSync(): Boolean = PeatJni.startSyncJni(handle)

    /**
     * Get all cells as JSON array string.
     * @return JSON array of cell objects
     */
    fun getCellsJson(): String = PeatJni.getCellsJni(handle)

    /**
     * Get all tracks as JSON array string.
     * @return JSON array of track objects
     */
    fun getTracksJson(): String = PeatJni.getTracksJni(handle)

    /**
     * Get all nodes as JSON array string.
     * @return JSON array of node objects
     */
    fun getNodesJson(): String = PeatJni.getNodesJni(handle)

    /**
     * Publish a node (self-position/PLI) to the Peat network.
     * @param nodeJson JSON string representing the node data
     * @return true if published successfully
     */
    fun publishNode(nodeJson: String): Boolean = PeatJni.publishNodeJni(handle, nodeJson)

    /**
     * Publish a generic document into a named collection. The JSON's
     * `"id"` field is optional; when absent (or non-string) the backend
     * assigns one. Returns the document id actually used — caller-
     * supplied or backend-assigned — or empty string on failure. See
     * [PeatJni.publishDocumentJni] for full contract.
     */
    fun publishDocument(collection: String, json: String): String =
        PeatJni.publishDocumentJni(handle, collection, json)

    /**
     * Ingest a peat-btle BLE position envelope through the cross-transport
     * BLE gateway. Translated to a track document and published; iroh-bound
     * peers receive it via Automerge sync. Returns the track document id on
     * success, or empty string on failure. See [PeatJni.ingestPositionJni]
     * for full envelope shape and failure modes.
     */
    fun ingestPosition(json: String): String =
        PeatJni.ingestPositionJni(handle, json)

    /**
     * Connect to a known peer by node ID and address (bypasses mDNS).
     * @param nodeId Hex-encoded Iroh node ID
     * @param address Socket address (e.g. "192.168.1.100:42009")
     * @return true if connection initiated successfully
     */
    fun connectPeer(nodeId: String, address: String): Boolean =
        PeatJni.connectPeerJni(handle, nodeId, address)

    // ========================================================================
    // BLE Transport Methods (ADR-047 Android Bootstrap)
    // ========================================================================

    /**
     * Signal BLE transport started/stopped to Rust TransportManager.
     * @param started true when BLE stack is ready, false on shutdown
     */
    fun bleSetStarted(started: Boolean) = PeatJni.bleSetStartedJni(handle, started)

    /**
     * Add a reachable BLE peer for PACE routing.
     * @param peerId Peer ID as 8-char hex string (e.g. "0A1B2C3D")
     */
    fun bleAddPeer(peerId: String) = PeatJni.bleAddPeerJni(handle, peerId)

    /**
     * Remove a reachable BLE peer from PACE routing.
     * @param peerId Peer ID as 8-char hex string (e.g. "0A1B2C3D")
     */
    fun bleRemovePeer(peerId: String) = PeatJni.bleRemovePeerJni(handle, peerId)

    /**
     * Query whether BLE transport is available (started) in Rust TransportManager.
     * @return true if BLE transport is active
     */
    fun bleIsAvailable(): Boolean = PeatJni.bleIsAvailableJni(handle)

    /**
     * Get the number of reachable BLE peers known to Rust TransportManager.
     * @return Number of BLE peers
     */
    fun blePeerCount(): Int = PeatJni.blePeerCountJni(handle)

    /**
     * Free the native node resources.
     */
    override fun close() {
        Log.d(TAG, "Closing Peat node handle: $handle")
        PeatJni.freeNodeJni(handle)
    }
}
