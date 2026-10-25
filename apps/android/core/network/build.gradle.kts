plugins {
    alias(libs.plugins.l2l.android.library)
}

dependencies {
    api(projects.core.protocol)
    implementation(projects.core.common)
    implementation(libs.kotlinx.coroutines.core)
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
