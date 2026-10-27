plugins {
    alias(libs.plugins.l2l.android.library)
    alias(libs.plugins.l2l.hilt)
}

dependencies {
    api(projects.core.data)
    implementation(projects.core.clipboard)
    implementation(projects.core.files)
    implementation(projects.core.notifications)
    implementation(projects.core.photos)
    implementation(projects.core.ring)
    implementation(projects.core.status)
    api(projects.core.protocol)
    api(projects.core.security)
    implementation(libs.conscrypt.android)
    implementation(libs.kotlinx.coroutines.core)

    testImplementation(projects.core.testing)
    // The QR code parser, for the interop test with the desktop's code.
    testImplementation(projects.core.discovery)
    // Unit tests run on the desktop JVM, where conscrypt-android cannot load
    // its native library. The OpenJDK build of the same release has the
    // same Java API and the same BoringSSL, so the TLS tests exercise the
    // code the phone runs. Apache-2.0, like conscrypt-android.
    testRuntimeOnly(libs.conscrypt.openjdk.uber)
}

configurations.matching { it.name.endsWith("UnitTestRuntimeClasspath") }.configureEach {
    exclude(group = "org.conscrypt", module = "conscrypt-android")
}

// Byte-level wire-format vectors shared with the desktop tests. Framing tests
// check against them when they are present in the checkout.
val sharedTestData: File = rootProject.file("../../proto/testdata")
tasks.withType<Test>().configureEach {
    inputs.files(fileTree(sharedTestData))
        .withPropertyName("sharedTestData")
        .withPathSensitivity(PathSensitivity.RELATIVE)
    systemProperty("l2l.testdata.dir", sharedTestData.absolutePath)
}

// The desktop core's interop_desktop example, built by scripts/interop.sh.
// DesktopInteropTest runs against it and is skipped when it is not set.
val interopDesktop: String? = providers.environmentVariable("L2L_INTEROP_DESKTOP").orNull
tasks.withType<Test>().configureEach {
    if (interopDesktop != null) {
        inputs.file(interopDesktop).withPropertyName("interopDesktop").withPathSensitivity(PathSensitivity.NONE)
        systemProperty("l2l.interop.desktop", interopDesktop)
    }
}
