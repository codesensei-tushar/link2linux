package dev.link2linux.android.core.common

import javax.inject.Qualifier

/** Marks an injected `CoroutineDispatcher` so tests can swap it out. */
@Qualifier
@Retention(AnnotationRetention.RUNTIME)
annotation class Dispatcher(val dispatcher: L2lDispatchers)

enum class L2lDispatchers {
    Default,
    IO,
}
