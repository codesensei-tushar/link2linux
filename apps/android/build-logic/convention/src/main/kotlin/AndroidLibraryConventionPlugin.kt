import com.android.build.api.dsl.LibraryExtension
import dev.link2linux.android.buildlogic.Sdk
import dev.link2linux.android.buildlogic.configureKotlinAndroid
import dev.link2linux.android.buildlogic.defaultNamespace
import dev.link2linux.android.buildlogic.libs
import dev.link2linux.android.buildlogic.library
import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.kotlin.dsl.configure
import org.gradle.kotlin.dsl.dependencies

class AndroidLibraryConventionPlugin : Plugin<Project> {
    override fun apply(target: Project) {
        with(target) {
            pluginManager.apply("com.android.library")

            extensions.configure<LibraryExtension> {
                configureKotlinAndroid(this)
                namespace = defaultNamespace
                defaultConfig.testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
                testOptions.targetSdk = Sdk.TARGET
                lint.targetSdk = Sdk.TARGET
            }

            dependencies {
                add("testImplementation", libs.library("junit"))
            }
        }
    }
}
