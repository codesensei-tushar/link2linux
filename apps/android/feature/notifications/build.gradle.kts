plugins {
    alias(libs.plugins.l2l.android.feature)
}

dependencies {
    // The engine and its transport interface; the session wires them (see NotificationsRuntime).
    api(projects.core.notifications)

    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.kotlinx.coroutines.core)

    testImplementation(libs.kotlinx.coroutines.test)
}
