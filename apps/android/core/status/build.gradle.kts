plugins {
    alias(libs.plugins.l2l.jvm.library)
}

// The platform-neutral half of the phone status feature: it shares the
// platform's readings among the desktops that follow them, and paces each
// desktop's updates. It has no Android dependency, so it is covered by JVM
// tests; feature/status provides the Android side.
dependencies {
    api(projects.core.protocol)
    api(libs.kotlinx.coroutines.core)

    testImplementation(libs.kotlinx.coroutines.test)
}
