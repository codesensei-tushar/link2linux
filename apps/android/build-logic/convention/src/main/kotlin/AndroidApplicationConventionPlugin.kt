import com.android.build.api.dsl.ApplicationExtension
import dev.link2linux.android.buildlogic.Sdk
import dev.link2linux.android.buildlogic.configureKotlinAndroid
import dev.link2linux.android.buildlogic.libs
import dev.link2linux.android.buildlogic.library
import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.kotlin.dsl.configure
import org.gradle.kotlin.dsl.dependencies

class AndroidApplicationConventionPlugin : Plugin<Project> {
    override fun apply(target: Project) {
        with(target) {
            pluginManager.apply("com.android.application")

            extensions.configure<ApplicationExtension> {
                configureKotlinAndroid(this)
                defaultConfig.targetSdk = Sdk.TARGET
                defaultConfig.testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
            }

            dependencies {
                add("testImplementation", libs.library("junit"))
            }
        }
    }
}
