package dev.link2linux.android.buildlogic

import org.gradle.api.Project
import org.gradle.api.artifacts.MinimalExternalModuleDependency
import org.gradle.api.artifacts.VersionCatalog
import org.gradle.api.artifacts.VersionCatalogsExtension
import org.gradle.api.provider.Provider
import org.gradle.kotlin.dsl.getByType

/** The `gradle/libs.versions.toml` catalog, for use inside convention plugins. */
internal val Project.libs: VersionCatalog
    get() = extensions.getByType<VersionCatalogsExtension>().named("libs")

internal fun VersionCatalog.library(alias: String): Provider<MinimalExternalModuleDependency> =
    findLibrary(alias).orElseThrow { IllegalStateException("No library '$alias' in libs.versions.toml") }

/**
 * Namespace derived from the Gradle path, so `:core:designsystem` becomes
 * `dev.link2linux.android.core.designsystem`.
 */
internal val Project.defaultNamespace: String
    get() = "dev.link2linux.android" + path.replace(':', '.').replace('-', '_')
