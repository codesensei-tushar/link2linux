plugins {
    alias(libs.plugins.l2l.android.library)
}

dependencies {
    api(projects.core.data)
    api(projects.core.model)
    implementation(libs.androidx.core.ktx)
    implementation(libs.kotlinx.coroutines.core)

    testImplementation(libs.kotlinx.coroutines.test)
}

// Pairing and discovery vector shared with the desktop tests. The tag and
// beacon tests check against it when it is present in the checkout.
val sharedTestData: File = rootProject.file("../../proto/testdata")
tasks.withType<Test>().configureEach {
    inputs.files(fileTree(sharedTestData))
        .withPropertyName("sharedTestData")
        .withPathSensitivity(PathSensitivity.RELATIVE)
    systemProperty("l2l.testdata.dir", sharedTestData.absolutePath)
}
