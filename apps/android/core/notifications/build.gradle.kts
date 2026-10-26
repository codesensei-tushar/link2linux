plugins {
    alias(libs.plugins.l2l.jvm.library)
}

// The platform-neutral half of the notifications feature: it maps what the
// phone's notification listener reports onto the wire messages, keeps the
// current set with its change log, and answers the desktop's requests. It has
// no Android dependency, so it is covered by JVM tests; feature/notifications
// provides the Android side.
dependencies {
    api(projects.core.protocol)
    api(libs.kotlinx.coroutines.core)

    testImplementation(libs.kotlinx.coroutines.test)
}
