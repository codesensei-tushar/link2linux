import dev.link2linux.android.buildlogic.libs
import dev.link2linux.android.buildlogic.library
import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.kotlin.dsl.dependencies

/**
 * A `feature/<name>` module: a Compose library that exposes a Navigation 3
 * route (a serializable NavKey) and the screen behind it.
 */
class AndroidFeatureConventionPlugin : Plugin<Project> {
    override fun apply(target: Project) {
        with(target) {
            pluginManager.apply("l2l.android.library")
            pluginManager.apply("l2l.android.library.compose")
            pluginManager.apply("org.jetbrains.kotlin.plugin.serialization")

            dependencies {
                add("implementation", project(":core:designsystem"))
                add("implementation", project(":core:model"))
                add("implementation", libs.library("androidx-navigation3-runtime"))
                add("implementation", libs.library("kotlinx-serialization-core"))
            }
        }
    }
}
