plugins {
    alias(libs.plugins.l2l.android.feature)
    alias(libs.plugins.l2l.hilt)
}

dependencies {
    // The engine and its interfaces; the session wires them (see FilesRuntime).
    api(projects.core.files)
    implementation(projects.core.data)

    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.hilt.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.kotlinx.coroutines.core)

    testImplementation(libs.kotlinx.coroutines.test)
}
