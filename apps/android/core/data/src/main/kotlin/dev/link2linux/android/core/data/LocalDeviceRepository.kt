package dev.link2linux.android.core.data

import dev.link2linux.android.core.model.Device

/** This installation as it introduces itself in its `Hello`. */
interface LocalDeviceRepository {

    /**
     * This phone: a random id created on first use and kept until the app's
     * data is cleared, and the name the user chose or the default.
     */
    suspend fun localDevice(): Device

    /** Sets the name shown to desktops, at most 64 bytes of UTF-8; null restores the default. */
    suspend fun setName(name: String?)

    companion object {
        /** Longest device name, in bytes of UTF-8. */
        const val MAX_NAME_BYTES: Int = 64
    }
}
