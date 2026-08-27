package com.defenseunicorns.peat

import androidx.annotation.Keep

/**
 * Receives committed PEAT document changes from the native store.
 *
 * Callbacks run on a Rust runtime thread. Implementations must move Android UI
 * or host dispatcher work onto the appropriate host thread. A notification is
 * only a key; consumers read the current document through
 * [PeatJni.getDocumentJni].
 */
@Keep
interface DocumentChangeListener {
    fun onChange(collection: String, docId: String)

    fun onError(message: String)
}
