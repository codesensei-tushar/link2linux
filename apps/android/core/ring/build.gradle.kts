plugins {
    alias(libs.plugins.l2l.jvm.library)
}

// The platform-neutral half of the ring feature: it decides when the phone
// rings and when it stops, and which desktops hear that it stopped. It has
// no Android dependency, so it is covered by JVM tests; feature/ring
// provides the sound, the vibration and the notification.
dependencies {
    api(projects.core.protocol)
    api(libs.kotlinx.coroutines.core)

    testImplementation(libs.kotlinx.coroutines.test)
}
