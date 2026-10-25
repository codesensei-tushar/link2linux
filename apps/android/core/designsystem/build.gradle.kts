plugins {
    alias(libs.plugins.l2l.android.library)
    alias(libs.plugins.l2l.android.library.compose)
}

dependencies {
    api(libs.androidx.compose.material3)
    api(projects.core.model)
}
