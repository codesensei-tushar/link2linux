package dev.link2linux.android.di

import android.content.Context
import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.android.qualifiers.ApplicationContext
import dagger.hilt.components.SingletonComponent
import dev.link2linux.android.core.data.loggingUncaught
import dev.link2linux.android.core.network.features.ClipboardSessionHooks
import dev.link2linux.android.core.network.features.CompositeSessionHooks
import dev.link2linux.android.core.network.features.FileSessionHooks
import dev.link2linux.android.core.network.features.NotificationSessionHooks
import dev.link2linux.android.core.network.features.StatusSessionHooks
import dev.link2linux.android.core.network.pairing.SessionHooks
import dev.link2linux.android.feature.clipboard.ClipboardRuntime
import dev.link2linux.android.feature.files.FilesRuntime
import dev.link2linux.android.feature.notifications.NotificationsRuntime
import dev.link2linux.android.feature.status.StatusRuntime
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import javax.inject.Singleton

/** Plugs the features into the sessions with paired desktops. */
@Module
@InstallIn(SingletonComponent::class)
internal object RuntimeModule {

    @Provides
    @Singleton
    fun sessionHooks(
        @ApplicationContext context: Context,
        files: FilesRuntime,
        status: StatusRuntime,
    ): SessionHooks = CompositeSessionHooks(
        NotificationSessionHooks(
            engine = NotificationsRuntime.get(context).engine,
            // Closes the sessions the engine gives up on; the app outlives them all.
            scope = CoroutineScope(SupervisorJob() + Dispatchers.IO + loggingUncaught("the notification sessions")),
        ),
        ClipboardSessionHooks(ClipboardRuntime.get(context).engine),
        FileSessionHooks(
            engine = files.engine,
            // Each session's worker lives as long as the session; the app outlives them all.
            scope = CoroutineScope(SupervisorJob() + Dispatchers.IO + loggingUncaught("the file sessions")),
        ),
        StatusSessionHooks(
            engine = status.engine,
            // Each session's follower lives as long as the session; the app outlives them all.
            scope = CoroutineScope(SupervisorJob() + Dispatchers.Default + loggingUncaught("the status sessions")),
        ),
    )
}
