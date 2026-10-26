plugins {
    alias(libs.plugins.l2l.android.library)
    alias(libs.plugins.l2l.hilt)
}

dependencies {
    api(projects.core.data)
    api(projects.core.protocol)
    api(projects.core.security)
    implementation(libs.conscrypt.android)
    implementation(libs.kotlinx.coroutines.core)

    testImplementation(projects.core.testing)
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
