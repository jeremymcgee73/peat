package com.defenseunicorns.peat

/**
 * Default no-op `PeerEventManager`. Override at the consumer level
 * (place a class with the same FQN earlier on the classpath) to
 * observe peer events.
 *
 * peat-ffi's native code calls `notifyPeerConnected` /
 * `notifyPeerDisconnected` as **static** methods on this class
 * whenever the underlying iroh transport reports a peer state
 * change. Before 0.1.2, the class lookup was assumed to be
 * consumer-provided; if the class was missing, `JNI_OnLoad` left a
 * pending `ClassNotFoundException` on the JNI stack and the next
 * find_class aborted the process with SIGABRT (peat#887, fixed in
 * the Rust side of 0.1.2). Shipping this default eliminates the
 * crash class entirely — there's always a `PeerEventManager` to
 * find, even when the consumer doesn't care about peer events.
 *
 * Method signatures must stay in lockstep with `notify_peer_event`
 * in peat-ffi/src/lib.rs:
 *   notifyPeerConnected(String peerId)                    → (Ljava/lang/String;)V
 *   notifyPeerDisconnected(String peerId, String reason)  → (Ljava/lang/String;Ljava/lang/String;)V
 */
object PeerEventManager {
    @JvmStatic
    fun notifyPeerConnected(peerId: String) {
        // Default no-op. Consumer override via classpath precedence
        // (e.g. peat-atak-plugin) provides the real implementation.
    }

    @JvmStatic
    fun notifyPeerDisconnected(peerId: String, reason: String) {
        // Default no-op.
    }
}
