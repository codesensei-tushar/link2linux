package dev.link2linux.android.core.network.pairing

import dev.link2linux.protocol.v1.DeviceKind
import dev.link2linux.protocol.v1.Hello
import dev.link2linux.protocol.v1.HelloKt
import dev.link2linux.protocol.v1.hello
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class PeerHelloTest {

    private fun desktop(fill: HelloKt.Dsl.() -> Unit = {}): Hello = hello {
        protocolVersion = 1
        deviceId = DesktopPeer.DEVICE_ID
        deviceName = "Desk"
        kind = DeviceKind.DEVICE_KIND_DESKTOP
        features += "notifications"
        fill()
    }

    @Test
    fun aHelloWithinTheLimitsIsKeptAsItIs() {
        val hello = desktop()
        assertSame(hello, PeerHello.checked(hello))
    }

    @Test
    fun theDeviceIdIsUpTo64LettersDigitsAndHyphens() {
        for (id in listOf("desktop-0123456789abcdef0123456789abcdef", DesktopPeer.DEVICE_ID, "desk-1", "A".repeat(64))) {
            assertEquals(id, PeerHello.checked(desktop { deviceId = id }).deviceId)
        }
        for (id in listOf("", "a".repeat(65), "desk 1", "../desk", "{${DesktopPeer.DEVICE_ID}}", "désk", "desk\u202E")) {
            assertThrows(IllegalArgumentException::class.java) { PeerHello.checked(desktop { deviceId = id }) }
        }
    }

    @Test
    fun aLongNameIsCutAtACharacterBoundary() {
        val checked = PeerHello.checked(desktop { deviceName = "ü".repeat(2_048) })
        assertEquals("ü".repeat(32), checked.deviceName)
        assertTrue(PeerHello.checked(desktop { deviceName = "a" + "😀".repeat(16) }).deviceName.toByteArray().size <= 64)
    }

    @Test
    fun controlAndBidiCharactersAreDroppedFromTheName() {
        // U+202E would show "Desk‮txt.exe" reversed; emoji sequences keep their joiners.
        assertEquals("Desktxt.exe", PeerHello.checked(desktop { deviceName = "Desk‮txt.exe" }).deviceName)
        assertEquals("Desk two", PeerHello.checked(desktop { deviceName = "Desk\n\u0007 two⁦" }).deviceName)
        assertEquals("👩‍💻 Desk", PeerHello.checked(desktop { deviceName = "👩‍💻 Desk" }).deviceName)
    }

    @Test
    fun tooManyOrTooLongFeaturesAreRefused() {
        assertThrows(IllegalArgumentException::class.java) {
            PeerHello.checked(desktop { features += List(10_000) { "" } })
        }
        assertThrows(IllegalArgumentException::class.java) {
            PeerHello.checked(desktop { features += "x".repeat(33) })
        }
        PeerHello.checked(desktop { features += List(PeerHello.MAX_FEATURES - 1) { "f$it" } })
    }

    @Test
    fun aLongAppVersionIsCut() {
        assertEquals("1".repeat(32), PeerHello.checked(desktop { appVersion = "1".repeat(4_096) }).appVersion)
    }
}
