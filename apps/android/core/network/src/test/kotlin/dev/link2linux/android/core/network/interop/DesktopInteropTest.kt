package dev.link2linux.android.core.network.interop

import dev.link2linux.android.core.data.InMemoryPairedDeviceRepository
import dev.link2linux.android.core.data.ListenAddresses
import dev.link2linux.android.core.data.LocalDeviceRepository
import dev.link2linux.android.core.discovery.QrOfferParser
import dev.link2linux.android.core.discovery.QrParseResult
import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.PairingState
import dev.link2linux.android.core.network.pairing.PairingService
import dev.link2linux.android.core.network.tls.asSource
import dev.link2linux.android.core.security.Spki
import dev.link2linux.android.core.testing.TestCertificates
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import java.io.File
import java.net.InetAddress
import java.net.InetSocketAddress
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * The phone's listener and pairing against the desktop's own code: the
 * `interop_desktop` example of the Rust core, which dials with rustls while
 * this side answers with Conscrypt. Pairing only succeeds when both stacks
 * agree on the TLS exporter values and both implementations agree on the
 * proofs, the SAS code and the QR code format.
 *
 * Runs when the `l2l.interop.desktop` system property names the built
 * example, as scripts/interop.sh does; skipped otherwise.
 */
class DesktopInteropTest {

    private val binary = System.getProperty("l2l.interop.desktop")?.takeIf { it.isNotBlank() }?.let(::File)
    private val phone = TestCertificates.generate("interop phone")
    private val devices = InMemoryPairedDeviceRepository()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val local = object : LocalDeviceRepository {
        override suspend fun localDevice() = Device(DeviceId("phone-1"), "Phone", DeviceKind.PHONE)
        override suspend fun setName(name: String?) = Unit
    }
    private val service = PairingService(
        identity = phone.asSource(),
        devices = devices,
        localDevice = local,
        scope = scope,
        listenAddresses = ListenAddresses { it.isLoopbackAddress },
        port = 0,
        bindAddress = InetAddress.getLoopbackAddress(),
    )
    private var desktop: DesktopProcess? = null

    @Before
    fun requireDesktop() {
        assumeTrue("Set L2L_INTEROP_DESKTOP to the interop_desktop example (scripts/interop.sh)", binary != null)
        check(binary!!.canExecute()) { "$binary is not an executable" }
    }

    @After
    fun tearDown() {
        desktop?.close()
        runBlocking { service.stop() }
        scope.cancel()
    }

    private fun start(mode: String) = DesktopProcess(binary!!, mode).also { desktop = it }

    private suspend fun port(): Int = service.listeningPort.filterNotNull().first()

    private suspend fun awaitState(predicate: (PairingState) -> Boolean): PairingState =
        withTimeout(TIMEOUT_MILLIS) { service.state.first(predicate) }

    @Test
    fun qrCodePairingReconnectAndUnpair() = runBlocking<Unit> {
        val desktop = start("qr")
        val offer = when (val parsed = QrOfferParser.parse(desktop.expect("qr"))) {
            is QrParseResult.Parsed -> parsed.offer
            else -> failWith("The phone cannot read the desktop's QR code: $parsed")
        }
        assertEquals("Interop desk ü", offer.desktopName)
        assertEquals(listOf(InetSocketAddress(InetAddress.getByName("192.0.2.7"), 27381)), offer.beaconAddresses)
        assertEquals("0C:1A:2B:3C:4D:5E", offer.bluetoothAddress)

        service.startQrPairing(offer)
        desktop.send(port().toString())

        assertEquals("${Spki.fingerprint(phone.certificate).toBase64Url()} phone-1", desktop.expect("paired"))
        val paired = (awaitState { it is PairingState.Paired } as PairingState.Paired).device
        assertEquals(offer.desktopFingerprint, paired.fingerprint)
        assertEquals("interop-desktop", paired.device.id.value)
        assertEquals("Interop desk ü", paired.device.name)

        // A second connection, pinned on both sides, ends with the desktop
        // unpairing.
        desktop.expect("reconnected")
        desktop.expect("done")
        assertEquals(0, desktop.exitCode())
        withTimeout(TIMEOUT_MILLIS) { devices.pairedDevices.first { it.isEmpty() } }
    }

    @Test
    fun sasPairingShowsTheSameCodeOnBothSides() = runBlocking<Unit> {
        val desktop = start("sas")
        service.startSasPairing()
        desktop.send(port().toString())

        val shown = desktop.expect("code")
        val asked = awaitState { it is PairingState.ConfirmCode } as PairingState.ConfirmCode
        assertEquals(shown, asked.code.value.toString().padStart(6, '0'))
        service.confirmCode(true)

        assertEquals("${Spki.fingerprint(phone.certificate).toBase64Url()} phone-1", desktop.expect("paired"))
        val paired = (awaitState { it is PairingState.Paired } as PairingState.Paired).device
        assertEquals("interop-desktop", paired.device.id.value)
        desktop.expect("done")
        assertEquals(0, desktop.exitCode())
    }

    /** The desktop example, talked to in lines; its stderr goes to the test output. */
    private class DesktopProcess(binary: File, mode: String) : AutoCloseable {
        private val process = ProcessBuilder(binary.path, mode)
            .redirectError(ProcessBuilder.Redirect.INHERIT)
            .start()
        /** Each line the desktop printed; null text once its output ended. */
        private val lines = LinkedBlockingQueue<Line>()

        private class Line(val text: String?)

        init {
            Thread {
                process.inputStream.bufferedReader().forEachLine { lines.put(Line(it)) }
                lines.put(Line(null))
            }.apply { isDaemon = true }.start()
        }

        /** Waits for the next line, which must start with [word], and returns the rest. */
        fun expect(word: String): String {
            val line = lines.poll(TIMEOUT_MILLIS, TimeUnit.MILLISECONDS)?.let {
                it.text ?: failWith("The desktop exited with ${process.waitFor()} before \"$word\"; see its stderr")
            } ?: failWith("The desktop printed nothing for ${TIMEOUT_MILLIS / 1000} s; expected \"$word\"")
            if (line != word && !line.startsWith("$word ")) failWith("Expected \"$word\" from the desktop, got \"$line\"")
            return line.removePrefix(word).trimStart()
        }

        fun send(line: String) {
            process.outputStream.write("$line\n".toByteArray())
            process.outputStream.flush()
        }

        fun exitCode(): Int {
            if (!process.waitFor(TIMEOUT_MILLIS, TimeUnit.MILLISECONDS)) failWith("The desktop did not exit")
            return process.exitValue()
        }

        override fun close() {
            process.destroyForcibly()
        }
    }

    private companion object {
        const val TIMEOUT_MILLIS = 20_000L

        fun failWith(message: String): Nothing = throw AssertionError(message)
    }
}
