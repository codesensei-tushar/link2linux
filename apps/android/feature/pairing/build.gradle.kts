plugins {
    alias(libs.plugins.l2l.android.feature)
    alias(libs.plugins.l2l.hilt)
}

dependencies {
    implementation(projects.core.data)
    implementation(projects.core.discovery)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.hilt.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)

    testImplementation(libs.kotlinx.coroutines.test)
}
