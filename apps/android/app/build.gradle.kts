plugins {
    alias(libs.plugins.l2l.android.application)
    alias(libs.plugins.l2l.android.application.compose)
    alias(libs.plugins.l2l.hilt)
}

android {
    namespace = "dev.link2linux.android"

    defaultConfig {
        applicationId = "dev.link2linux.android"
        versionCode = 7
        versionName = "0.4.1"
    }
}

dependencies {
    implementation(projects.core.common)
    implementation(projects.core.data)
    implementation(projects.core.designsystem)
    implementation(projects.core.discovery)
    implementation(projects.core.model)
    implementation(projects.core.network)
    implementation(projects.feature.calls)
    implementation(projects.feature.clipboard)
    implementation(projects.feature.files)
    implementation(projects.feature.notifications)
    implementation(projects.feature.pairing)
    implementation(projects.feature.photos)
    implementation(projects.feature.ring)
    implementation(projects.feature.screen)
    implementation(projects.feature.status)

    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.lifecycle.viewmodel.navigation3)
    implementation(libs.androidx.navigation3.runtime)
    implementation(libs.androidx.navigation3.ui)
}
