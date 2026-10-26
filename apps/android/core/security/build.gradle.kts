plugins {
    alias(libs.plugins.l2l.android.library)
}

dependencies {
    api(projects.core.model)

    testImplementation(projects.core.testing)
}

// Shared test vectors (proto/testdata), checked by the desktop tests too.
val sharedTestData: File = rootProject.file("../../proto/testdata")
tasks.withType<Test>().configureEach {
    inputs.files(fileTree(sharedTestData))
        .withPropertyName("sharedTestData")
        .withPathSensitivity(PathSensitivity.RELATIVE)
    systemProperty("l2l.testdata.dir", sharedTestData.absolutePath)
}
