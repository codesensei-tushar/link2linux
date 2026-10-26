plugins {
    alias(libs.plugins.l2l.android.application)
    alias(libs.plugins.l2l.android.application.compose)
    alias(libs.plugins.l2l.hilt)
}

android {
    namespace = "dev.link2linux.android"

    defaultConfig {
        applicationId = "dev.link2linux.android"
        versionCode = 2
        versionName = "0.1.1"
    }
}

dependencies {
    implementation(projects.core.data)
    implementation(projects.core.designsystem)
    implementation(projects.core.discovery)
    implementation(projects.core.model)
    implementation(projects.core.network)
    implementation(projects.feature.calls)
    implementation(projects.feature.clipboard)
    implementation(projects.feature.files)
    implementation(projects.feature.messages)
    implementation(projects.feature.notifications)
    implementation(projects.feature.pairing)
    implementation(projects.feature.photos)
    implementation(projects.feature.screen)

    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.lifecycle.viewmodel.navigation3)
    implementation(libs.androidx.navigation3.runtime)
    implementation(libs.androidx.navigation3.ui)
}
