package dev.link2linux.android.core.discovery.android

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import dev.link2linux.android.core.discovery.Announcement
import dev.link2linux.android.core.discovery.DiscoveryConstants
import dev.link2linux.android.core.discovery.ServiceAdvertiser
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Registers the `_link2linux._tcp` service with [NsdManager]. Each
 * registration uses a new listener, because Android refuses to reuse one
 * that was registered before; replacing a registration waits for the old one
 * to be gone first.
 */
class NsdAdvertiser(context: Context) : ServiceAdvertiser {

    private val nsd = context.applicationContext.getSystemService(NsdManager::class.java)
    private val mutex = Mutex()
    private var current: Registration? = null

    private val _registeredName = MutableStateFlow<String?>(null)
    override val registeredName: StateFlow<String?> = _registeredName.asStateFlow()

    private class Registration(val listener: NsdManager.RegistrationListener, val gone: CompletableDeferred<Unit>)

    override suspend fun register(announcement: Announcement, port: Int) = mutex.withLock {
        unregisterLocked()
        val info = NsdServiceInfo().apply {
            serviceName = announcement.instanceName
            serviceType = DiscoveryConstants.SERVICE_TYPE
            this.port = port
            announcement.txt.forEach { (k, v) -> setAttribute(k, v) }
        }
        val registered = CompletableDeferred<String?>()
        val gone = CompletableDeferred<Unit>()
        val listener = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(serviceInfo: NsdServiceInfo) {
                registered.complete(unescapeName(serviceInfo.serviceName))
            }

            override fun onRegistrationFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {
                registered.complete(null)
                gone.complete(Unit)
            }

            override fun onServiceUnregistered(serviceInfo: NsdServiceInfo) {
                gone.complete(Unit)
            }

            override fun onUnregistrationFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {
                gone.complete(Unit)
            }
        }
        current = Registration(listener, gone)
        try {
            if (Build.VERSION.SDK_INT >= 33) {
                nsd.registerService(info, NsdManager.PROTOCOL_DNS_SD, Runnable::run, listener)
            } else {
                @Suppress("DEPRECATION")
                nsd.registerService(info, NsdManager.PROTOCOL_DNS_SD, listener)
            }
        } catch (_: RuntimeException) {
            current = null
            _registeredName.value = null
            return@withLock
        }
        _registeredName.value = withTimeoutOrNull(REGISTER_TIMEOUT_MILLIS) { registered.await() }
    }

    override suspend fun unregister() = mutex.withLock { unregisterLocked() }

    private suspend fun unregisterLocked() {
        val registration = current ?: return
        current = null
        _registeredName.value = null
        try {
            nsd.unregisterService(registration.listener)
        } catch (_: IllegalArgumentException) {
            return // never registered, or already gone
        }
        withTimeoutOrNull(REGISTER_TIMEOUT_MILLIS) { registration.gone.await() }
    }

    internal companion object {
        const val REGISTER_TIMEOUT_MILLIS = 5_000L

        /**
         * Android's older mDNS daemon (before API 34) escapes characters in
         * instance names, such as a space as `\032`, when it renames a
         * service after a conflict.
         */
        fun unescapeName(name: String): String =
            Regex("""\\(\d{3})""").replace(name) { match -> match.groupValues[1].toInt().toChar().toString() }
    }
}
