package dev.link2linux.android.core.discovery

import kotlinx.coroutines.flow.StateFlow

/**
 * The user's choice to keep link2linux connected in the background. Turning
 * it off stops the connection service and keeps it from starting again until
 * the user turns it on.
 */
interface ConnectionSwitch {

    val enabled: StateFlow<Boolean>

    fun setEnabled(enabled: Boolean)
}
