package dev.link2linux.android.feature.files

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.Uri
import com.google.protobuf.ByteString
import dagger.hilt.EntryPoint
import dagger.hilt.InstallIn
import dagger.hilt.android.EntryPointAccessors
import dagger.hilt.components.SingletonComponent
import dev.link2linux.android.core.files.Direction
import dev.link2linux.android.core.files.TransferId
import dev.link2linux.android.core.files.TransferKey

/**
 * Cancel on a transfer's progress notification. Not exported: only the
 * app's own PendingIntents reach it.
 */
class TransferActionReceiver : BroadcastReceiver() {

    @EntryPoint
    @InstallIn(SingletonComponent::class)
    internal interface Files {
        fun runtime(): FilesRuntime
    }

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ACTION_CANCEL) return
        val key = intent.data?.let(::keyOf) ?: return
        EntryPointAccessors.fromApplication(context.applicationContext, Files::class.java).runtime().cancel(key)
    }

    internal companion object {
        private const val ACTION_CANCEL = "dev.link2linux.android.files.CANCEL"
        private const val SCHEME = "link2linux-transfer"

        /**
         * The key travels as the intent's data, which also keeps each
         * transfer's PendingIntent apart from the others'.
         */
        fun cancelIntent(context: Context, key: TransferKey): Intent = Intent(context, TransferActionReceiver::class.java)
            .setAction(ACTION_CANCEL)
            .setData(
                Uri.Builder().scheme(SCHEME)
                    .appendPath(key.direction.name)
                    .appendPath(key.peerId)
                    .appendPath(key.id.toString())
                    .build(),
            )

        fun keyOf(uri: Uri): TransferKey? {
            if (uri.scheme != SCHEME) return null
            val (direction, peer, id) = uri.pathSegments.takeIf { it.size == 3 } ?: return null
            val bytes = id.takeIf { it.length == TransferId.SIZE * 2 }?.chunked(2)?.map { it.toIntOrNull(16) ?: return null } ?: return null
            return TransferKey(
                Direction.entries.firstOrNull { it.name == direction } ?: return null,
                peer,
                TransferId(ByteString.copyFrom(ByteArray(bytes.size) { bytes[it].toByte() })),
            )
        }
    }
}
