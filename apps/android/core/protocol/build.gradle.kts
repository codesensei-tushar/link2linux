import com.google.protobuf.gradle.proto

plugins {
    alias(libs.plugins.l2l.jvm.library)
    alias(libs.plugins.protobuf)
}

// Generated from the one schema at the repository root (proto/), shared with
// the desktop. The .proto files are never copied into this module.
sourceSets {
    main {
        proto {
            srcDir(rootProject.file("../../proto"))
        }
    }
}

protobuf {
    protoc {
        artifact = libs.protobuf.protoc.get().toString()
    }
    generateProtoTasks {
        all().configureEach {
            builtins {
                named("java") { option("lite") }
                create("kotlin") { option("lite") }
            }
        }
    }
}

dependencies {
    api(libs.protobuf.kotlin.lite)
    implementation(projects.core.model)
}
