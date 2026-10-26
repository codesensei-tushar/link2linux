plugins {
    alias(libs.plugins.l2l.jvm.library)
}

// The platform-neutral half of the clipboard feature: it puts the desktops'
// pushes on the clipboard through a port, and sends what the user shares to
// each desktop, one push at a time. It has no Android dependency, so it is
// covered by JVM tests; feature/clipboard provides the Android side.
dependencies {
    api(projects.core.protocol)
    api(libs.kotlinx.coroutines.core)

    testImplementation(libs.kotlinx.coroutines.test)
}
