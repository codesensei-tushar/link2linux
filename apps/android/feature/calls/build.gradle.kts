plugins {
    alias(libs.plugins.l2l.android.feature)
    alias(libs.plugins.l2l.hilt)
}

dependencies {
    // FeatureAvailability; the session wires the feature (see CallsRuntime).
    api(projects.core.protocol)
    implementation(projects.core.data)

    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.hilt.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.kotlinx.coroutines.core)
}
