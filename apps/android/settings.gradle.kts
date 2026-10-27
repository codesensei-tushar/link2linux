pluginManagement {
    includeBuild("build-logic")
    repositories {
        google {
            content {
                includeGroupByRegex("com\\.android.*")
                includeGroupByRegex("com\\.google.*")
                includeGroupByRegex("androidx.*")
            }
        }
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google {
            content {
                includeGroupByRegex("com\\.android.*")
                includeGroupByRegex("com\\.google.*")
                includeGroupByRegex("androidx.*")
            }
        }
        mavenCentral()
    }
}

rootProject.name = "link2linux"
enableFeaturePreview("TYPESAFE_PROJECT_ACCESSORS")

include(":app")
include(":core:clipboard")
include(":core:common")
include(":core:data")
include(":core:designsystem")
include(":core:discovery")
include(":core:files")
include(":core:model")
include(":core:network")
include(":core:notifications")
include(":core:protocol")
include(":core:security")
include(":core:status")
include(":core:testing")
include(":feature:calls")
include(":feature:clipboard")
include(":feature:files")
include(":feature:notifications")
include(":feature:pairing")
include(":feature:photos")
include(":feature:screen")
include(":feature:status")
