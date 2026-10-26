package dev.link2linux.android.core.data

import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PairingFailure
import dev.link2linux.android.core.model.PairingMethod
import dev.link2linux.android.core.model.PairingState
import dev.link2linux.android.core.model.PresenceKey
import dev.link2linux.android.core.model.SasCode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ConnectionStatusTest {

    private fun key(n: Int) = KeyFingerprint.of(ByteArray(32) { n.toByte() })

    private fun desktop(id: String, key: Int, next: Int? = null) = PairedDevice(
        device = Device(DeviceId(id), id, DeviceKind.DESKTOP),
        fingerprint = key(key),
        pairedAtEpochMillis = 0,
        presenceKey = PresenceKey.of(ByteArray(32)),
        nextFingerprint = next?.let(::key),
    )

    @Test
    fun connectedDesktopsAreFoundByEitherOfTheirKeys() {
        val a = desktop("a", 1)
        val b = desktop("b", 2, next = 3)
        val c = desktop("c", 4)
        assertEquals(listOf(a, b), listOf(a, b, c).connectedIn(setOf(key(1), key(3))))
        assertTrue(listOf(a, b, c).connectedIn(emptySet()).isEmpty())
    }

    @Test
    fun theListenerRunsForAPairedDesktopWhileTheSwitchIsOn() {
        assertTrue(ConnectionPolicy.shouldListen(listOf(desktop("a", 1)), PairingState.Idle, switchOn = true, onLocalNetwork = true))
        assertFalse(ConnectionPolicy.shouldListen(listOf(desktop("a", 1)), PairingState.Idle, switchOn = false, onLocalNetwork = true))
        assertFalse(ConnectionPolicy.shouldListen(emptyList(), PairingState.Idle, switchOn = true, onLocalNetwork = true))
    }

    @Test
    fun aPairingUnderWayNeedsTheListenerEvenWithoutAPairedDesktop() {
        val verifying = PairingState.Verifying(PairingMethod.SAS, "x")
        assertTrue(ConnectionPolicy.shouldListen(emptyList(), verifying, switchOn = true, onLocalNetwork = true))
        assertTrue(ConnectionPolicy.shouldListen(emptyList(), PairingState.ConfirmCode(SasCode(123456), "x"), switchOn = true, onLocalNetwork = true))
        assertFalse(ConnectionPolicy.shouldListen(emptyList(), PairingState.Failed(PairingFailure.TIMED_OUT), switchOn = true, onLocalNetwork = true))
        assertFalse(ConnectionPolicy.shouldListen(emptyList(), verifying, switchOn = false, onLocalNetwork = true))
    }

    @Test
    fun theListenerClosesOffALocalNetwork() {
        val verifying = PairingState.Verifying(PairingMethod.SAS, "x")
        assertFalse(ConnectionPolicy.shouldListen(listOf(desktop("a", 1)), PairingState.Idle, switchOn = true, onLocalNetwork = false))
        assertFalse(ConnectionPolicy.shouldListen(emptyList(), verifying, switchOn = true, onLocalNetwork = false))
    }

    @Test
    fun theServiceRunsOnlyWithAPairingOrOneUnderWayAndTheSwitchOn() {
        val waiting = PairingState.AwaitingDesktop.Sas(untilEpochMillis = 0, attemptsLeft = 3)
        // A fresh install: nothing paired, nothing started.
        assertFalse(ConnectionPolicy.shouldRun(emptyList(), PairingState.Idle, switchOn = true))
        assertFalse(ConnectionPolicy.shouldRun(emptyList(), PairingState.Failed(PairingFailure.TIMED_OUT), switchOn = true))
        assertTrue(ConnectionPolicy.shouldRun(emptyList(), waiting, switchOn = true))
        assertTrue(ConnectionPolicy.shouldRun(listOf(desktop("a", 1)), PairingState.Idle, switchOn = true))
        // The user turned it off.
        assertFalse(ConnectionPolicy.shouldRun(listOf(desktop("a", 1)), PairingState.Idle, switchOn = false))
        assertFalse(ConnectionPolicy.shouldRun(emptyList(), waiting, switchOn = false))
    }
}
