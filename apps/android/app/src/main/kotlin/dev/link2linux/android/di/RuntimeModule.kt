package dev.link2linux.android.di

import android.content.Context
import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.android.qualifiers.ApplicationContext
import dagger.hilt.components.SingletonComponent
import dev.link2linux.android.core.data.loggingUncaught
import dev.link2linux.android.core.network.features.NotificationSessionHooks
import dev.link2linux.android.core.network.pairing.SessionHooks
import dev.link2linux.android.feature.notifications.NotificationsRuntime
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
    fun sessionHooks(@ApplicationContext context: Context): SessionHooks = NotificationSessionHooks(
        engine = NotificationsRuntime.get(context).engine,
        // Status updates for a session live as long as the session; the app outlives them all.
        scope = CoroutineScope(SupervisorJob() + Dispatchers.IO + loggingUncaught("the notification sessions")),
    )
}
