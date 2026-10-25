package dev.link2linux.android.buildlogic

import com.android.build.api.dsl.CommonExtension
import org.gradle.api.JavaVersion
import org.gradle.api.Project
import org.gradle.kotlin.dsl.configure
import org.jetbrains.kotlin.gradle.dsl.JvmTarget
import org.jetbrains.kotlin.gradle.dsl.KotlinAndroidProjectExtension
import org.jetbrains.kotlin.gradle.dsl.KotlinBaseExtension
import org.jetbrains.kotlin.gradle.dsl.KotlinJvmProjectExtension

/** SDK levels shared by every Android module. */
internal object Sdk {
    const val MIN = 26
    const val COMPILE = 36
    const val TARGET = 36
}

private val JAVA_VERSION = JavaVersion.VERSION_17
private val JVM_TARGET = JvmTarget.JVM_17

/** Android options shared by application and library modules. */
internal fun Project.configureKotlinAndroid(commonExtension: CommonExtension) {
    commonExtension.apply {
        compileSdk = Sdk.COMPILE
        defaultConfig.minSdk = Sdk.MIN
        compileOptions.sourceCompatibility = JAVA_VERSION
        compileOptions.targetCompatibility = JAVA_VERSION
    }
    configureKotlin<KotlinAndroidProjectExtension>()
}

private inline fun <reified T : KotlinBaseExtension> Project.configureKotlin() = configure<T> {
    // Pass -PwarningsAsErrors=true to fail the build on Kotlin warnings.
    val warningsAsErrors = providers.gradleProperty("warningsAsErrors")
        .map(String::toBoolean)
        .orElse(false)
    when (this) {
        is KotlinAndroidProjectExtension -> compilerOptions
        is KotlinJvmProjectExtension -> compilerOptions
        else -> error("Unsupported Kotlin extension ${T::class}")
    }.apply {
        jvmTarget.set(JVM_TARGET)
        allWarningsAsErrors.set(warningsAsErrors)
        freeCompilerArgs.add("-opt-in=kotlin.RequiresOptIn")
    }
}
