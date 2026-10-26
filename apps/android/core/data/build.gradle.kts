plugins {
    alias(libs.plugins.l2l.android.library)
    alias(libs.plugins.l2l.hilt)
}

dependencies {
    api(projects.core.model)
    api(libs.kotlinx.coroutines.core)
    implementation(projects.core.security)
}
