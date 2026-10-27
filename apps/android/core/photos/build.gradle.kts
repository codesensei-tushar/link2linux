plugins {
    alias(libs.plugins.l2l.jvm.library)
}

// The platform-neutral half of the photos feature: it answers each
// desktop's requests for the list, thumbnails and the photos' bytes, paces
// its list updates, and refuses any photo it did not list to that desktop.
// It has no Android dependency, so it is covered by JVM tests;
// feature/photos provides the Android side.
dependencies {
    api(projects.core.protocol)
    api(libs.kotlinx.coroutines.core)
    // The rules for file names, which photo names follow too.
    implementation(projects.core.files)

    testImplementation(libs.kotlinx.coroutines.test)
}
