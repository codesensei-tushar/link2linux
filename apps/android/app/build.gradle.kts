plugins {
    alias(libs.plugins.l2l.android.application)
    alias(libs.plugins.l2l.android.application.compose)
}

android {
    namespace = "dev.link2linux.android"

    defaultConfig {
        applicationId = "dev.link2linux.android"
        versionCode = 1
        versionName = "0.1.0"
    }
}

dependencies {
    implementation(projects.core.designsystem)

    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.compose.material3)
}
