plugins {
    alias(libs.plugins.l2l.jvm.library)
}

// The platform-neutral half of the files feature: it sends and receives
// transfers as the protocol describes, with storage, file reading and the
// session behind small interfaces. It has no Android dependency, so it is
// covered by JVM tests; feature/files provides the Android side.
dependencies {
    api(projects.core.protocol)
    api(libs.kotlinx.coroutines.core)

    testImplementation(libs.kotlinx.coroutines.test)
}
