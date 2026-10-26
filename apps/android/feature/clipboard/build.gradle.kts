plugins {
    alias(libs.plugins.l2l.android.feature)
}

dependencies {
    // The engine; the session wires it (see ClipboardRuntime).
    api(projects.core.clipboard)

    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.kotlinx.coroutines.core)
}
