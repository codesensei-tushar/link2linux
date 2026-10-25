// Plugins are declared here once, without applying them, so every module
// resolves the same versions. Modules apply them through build-logic.
plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.android.library) apply false
    alias(libs.plugins.android.lint) apply false
    alias(libs.plugins.compose) apply false
    // Also sets the Kotlin version behind AGP's built-in Kotlin support, which
    // must match the Compose compiler plugin.
    alias(libs.plugins.kotlin.jvm) apply false
    alias(libs.plugins.protobuf) apply false
}
