plugins {
    alias(libs.plugins.l2l.jvm.library)
}

// The platform-neutral half of the screen feature: when the phone shares
// its screen and with which desktop, how the frames queue on their way out,
// and what the desktop's input does. It has no Android dependency, so it is
// covered by JVM tests; feature/screen provides the capture, the encoder,
// the user's consent and the accessibility service.
dependencies {
    api(projects.core.protocol)
    api(libs.kotlinx.coroutines.core)

    testImplementation(libs.kotlinx.coroutines.test)
}
