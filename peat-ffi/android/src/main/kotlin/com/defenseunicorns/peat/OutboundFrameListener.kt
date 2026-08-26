package com.defenseunicorns.peat

import androidx.annotation.Keep

/**
 * Receives Rust-encoded outbound frames from the canonical PEAT transport
 * fan-out. Callbacks run on a Rust runtime thread; implementations must hand
 * work to their own executor before touching Android radio or UI state.
 *
 * The byte payload is opaque to Kotlin. Protocol semantics, serialization,
 * authentication, deduplication, and relay behavior remain Rust-owned.
 */
@Keep
fun interface OutboundFrameListener {
    fun onFrame(transportId: String, collection: String, bytes: ByteArray)
}
