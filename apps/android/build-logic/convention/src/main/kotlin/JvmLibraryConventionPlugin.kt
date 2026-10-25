import dev.link2linux.android.buildlogic.configureKotlinJvm
import dev.link2linux.android.buildlogic.libs
import dev.link2linux.android.buildlogic.library
import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.kotlin.dsl.dependencies

/** Plain Kotlin/JVM module: no Android APIs, unit tests run on the JVM. */
class JvmLibraryConventionPlugin : Plugin<Project> {
    override fun apply(target: Project) {
        with(target) {
            pluginManager.apply("org.jetbrains.kotlin.jvm")
            pluginManager.apply("com.android.lint")
            configureKotlinJvm()

            dependencies {
                add("testImplementation", libs.library("junit"))
            }

            // CI and the Makefile run `testDebugUnitTest` across the whole build.
            // JVM modules have no build variants, so map that name onto `test`.
            tasks.register("testDebugUnitTest") {
                group = "verification"
                description = "Runs the JVM unit tests (alias of `test`)."
                dependsOn("test")
            }
        }
    }
}
