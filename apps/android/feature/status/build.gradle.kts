plugins {
    alias(libs.plugins.l2l.android.feature)
    alias(libs.plugins.l2l.hilt)
}

dependencies {
    // The engine; the session wires it (see StatusRuntime).
    api(projects.core.status)
    implementation(projects.core.data)

    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.hilt.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.kotlinx.coroutines.core)
}
