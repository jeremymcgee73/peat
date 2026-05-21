plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
    id("maven-publish")
    id("signing")
}

group = "com.defenseunicorns"
// 0.1.1 (was 0.1.0): first publish since the initial Maven Central
// drop. Additive — two new JNI methods added in peat-ffi 0.2.4
// (peat#879 / peat-mesh#138 M4): `endpointSocketAddrJni`,
// `getDocumentJni`. No removed or renamed symbols. Pre-1.0 patch
// bump signals additive-only; bump to 0.2.x when the next breaking
// JNI change lands.
//
// 0.1.2 (was 0.1.1): patch release closing peat#885 + peat#886 +
// peat#887. Additive across the JNI surface (forceStoreErrorForTestingJni)
// AND the AAR contract (PeatJni.kt + PeerEventManager.kt now shipped
// in the artifact). The Rust-side env.exception_clear() fix removes
// the SIGABRT consumers without a PeerEventManager were hitting
// at System.loadLibrary.
//
// Consumers should drop hand-rolled PeatJni / PeerEventManager
// declarations on bump (the AAR's classpath copies replace them
// canonically). peat-mesh#145 was the first consumer to take this
// path. peat-atak-plugin can migrate at its own pace; classpath
// precedence keeps both copies working until then.
version = "0.1.2"

android {
    namespace = "com.defenseunicorns.peat.ffi"
    compileSdk = 34

    defaultConfig {
        minSdk = 26
        targetSdk = 34
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        consumerProguardFiles("consumer-rules.pro")
        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    sourceSets {
        getByName("main") {
            jniLibs.srcDirs("src/main/jniLibs")
            // Canonical Kotlin bindings shipped in the AAR (peat#886).
            // Before 0.1.2, consumers had to hand-roll their own
            // PeatJni declarations (peat-atak-plugin had one;
            // peat-mesh#145's M4b had another). Divergence between
            // those copies and peat-ffi's actual extern fn surface
            // didn't surface until instrumented-test runtime — the
            // foot-gun peat-mesh#145 QA called out as the binding-
            // drift WARNING. 0.1.2 ships a canonical copy here so
            // every consumer imports the same source of truth.
            java.srcDir("src/main/kotlin")
        }
        getByName("androidTest") {
            // Instrumented tests live alongside the AAR sources so
            // peat-ffi's JNI surface gets surface-tier coverage in
            // its own repo (not just downstream in peat-mesh).
            // Driven by ci.yml's android-test job on the
            // peat-arm64-linux-gb10 self-hosted runner; mirrors
            // peat-mesh#145's pattern. peat#888 / peat#885.
            java.srcDir("src/androidTest/kotlin")
        }
    }

    publishing {
        singleVariant("release") {
            withSourcesJar()
        }
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.12.0")
    implementation("androidx.annotation:annotation:1.7.1")
    implementation("net.java.dev.jna:jna:5.14.0@aar")

    // Instrumented tests (peat#888, surface-tier coverage of the
    // forceStoreErrorForTestingJni / getDocumentJni throw contract).
    // Runs on the self-hosted peat-arm64-linux-gb10 runner via the
    // new android-test workflow.
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation("androidx.test:runner:1.6.2")
}

// Build native libraries using cargo from the workspace root
tasks.register<Exec>("buildNativeLibs") {
    description = "Build native Rust libraries for Android"
    group = "build"

    // peat workspace root is two levels up from peat-ffi/android/
    val workspaceRoot = rootProject.projectDir.parentFile.parentFile
    workingDir = workspaceRoot

    val ndkPath = System.getenv("ANDROID_NDK_HOME")
        ?: System.getenv("NDK_HOME")
        ?: "${System.getenv("ANDROID_HOME")}/ndk/27.0.12077973"

    val ndkToolchain = "$ndkPath/toolchains/llvm/prebuilt/linux-x86_64/bin"
    environment("ANDROID_NDK_HOME", ndkPath)
    environment("PATH", "$ndkToolchain:${System.getenv("PATH")}")
    // Linker for cargo
    environment("CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER", "$ndkToolchain/aarch64-linux-android26-clang")
    environment("CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER", "$ndkToolchain/armv7a-linux-androideabi26-clang")
    environment("CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER", "$ndkToolchain/x86_64-linux-android26-clang")
    // C compiler for cc-rs (ring crate)
    environment("CC_aarch64-linux-android", "$ndkToolchain/aarch64-linux-android26-clang")
    environment("CC_armv7-linux-androideabi", "$ndkToolchain/armv7a-linux-androideabi26-clang")
    environment("CC_x86_64-linux-android", "$ndkToolchain/x86_64-linux-android26-clang")
    environment("AR_aarch64-linux-android", "$ndkToolchain/llvm-ar")
    environment("AR_armv7-linux-androideabi", "$ndkToolchain/llvm-ar")
    environment("AR_x86_64-linux-android", "$ndkToolchain/llvm-ar")

    commandLine("bash", "-c", """
        set -e
        echo "Building peat-ffi native libraries from: ${'$'}(pwd)"

        # Build for arm64-v8a (modern Android devices)
        echo "Building for aarch64-linux-android (arm64-v8a)..."
        cargo build --release --lib -p peat-ffi --target aarch64-linux-android
        mkdir -p peat-ffi/android/src/main/jniLibs/arm64-v8a
        cp target/aarch64-linux-android/release/libpeat_ffi.so peat-ffi/android/src/main/jniLibs/arm64-v8a/

        # Build for armeabi-v7a (older devices)
        echo "Building for armv7-linux-androideabi (armeabi-v7a)..."
        cargo build --release --lib -p peat-ffi --target armv7-linux-androideabi
        mkdir -p peat-ffi/android/src/main/jniLibs/armeabi-v7a
        cp target/armv7-linux-androideabi/release/libpeat_ffi.so peat-ffi/android/src/main/jniLibs/armeabi-v7a/

        # Build for x86_64 (emulators)
        echo "Building for x86_64-linux-android (x86_64)..."
        cargo build --release --lib -p peat-ffi --target x86_64-linux-android
        mkdir -p peat-ffi/android/src/main/jniLibs/x86_64
        cp target/x86_64-linux-android/release/libpeat_ffi.so peat-ffi/android/src/main/jniLibs/x86_64/

        echo "Native libraries built successfully!"
    """.trimIndent())
}

// Generate Kotlin bindings from UniFFI
tasks.register<Exec>("generateBindings") {
    description = "Generate Kotlin bindings from UniFFI"
    group = "build"
    dependsOn("buildNativeLibs")

    val workspaceRoot = rootProject.projectDir.parentFile.parentFile
    workingDir = workspaceRoot

    commandLine("bash", "-c", """
        set -e
        echo "Generating Kotlin bindings..."
        cargo run -p peat-ffi --bin uniffi-bindgen generate \
            --library target/aarch64-linux-android/release/libpeat_ffi.so \
            --language kotlin \
            --out-dir peat-ffi/android/src/main/java
        echo "Kotlin bindings generated."
    """.trimIndent())
}

// Combined: build native + assemble AAR
tasks.register("buildAar") {
    description = "Build native libraries and assemble AAR"
    group = "build"
    dependsOn("buildNativeLibs")
    finalizedBy("assembleRelease")
}

// Publishing
afterEvaluate {
    publishing {
        publications {
            register<MavenPublication>("release") {
                groupId = "com.defenseunicorns"
                artifactId = "peat-ffi"
                version = project.version.toString()
                from(components["release"])

                pom {
                    name.set("Peat FFI Android")
                    description.set("Android bindings for Peat mesh protocol via UniFFI")
                    url.set("https://github.com/defenseunicorns/peat")

                    licenses {
                        license {
                            name.set("Apache License 2.0")
                            url.set("https://www.apache.org/licenses/LICENSE-2.0")
                        }
                    }

                    developers {
                        developer {
                            id.set("defenseunicorns")
                            name.set("Defense Unicorns")
                            email.set("oss@defenseunicorns.com")
                        }
                    }

                    scm {
                        connection.set("scm:git:git://github.com/defenseunicorns/peat.git")
                        developerConnection.set("scm:git:ssh://github.com/defenseunicorns/peat.git")
                        url.set("https://github.com/defenseunicorns/peat")
                    }
                }
            }
        }

        repositories {
            maven {
                name = "local"
                url = uri(layout.buildDirectory.dir("repo"))
            }
        }
    }

    signing {
        val signingKey = findProperty("signingInMemoryKey") as String? ?: System.getenv("ORG_GRADLE_PROJECT_signingInMemoryKey")
        val signingPassword = findProperty("signingInMemoryKeyPassword") as String? ?: System.getenv("ORG_GRADLE_PROJECT_signingInMemoryKeyPassword")
        if (signingKey != null && signingPassword != null) {
            useInMemoryPgpKeys(signingKey, signingPassword)
        } else {
            useGpgCmd()
        }
        sign(publishing.publications["release"])
    }
}

// Bundle for Maven Central upload
tasks.register<Zip>("createMavenCentralBundle") {
    description = "Create Maven Central bundle ZIP"
    group = "publishing"
    dependsOn("publishReleasePublicationToLocalRepository")

    from(layout.buildDirectory.dir("repo"))
    archiveFileName.set("peat-ffi-${project.version}-bundle.zip")
    destinationDirectory.set(layout.buildDirectory.dir("bundle"))
}

// Note: the publishToMavenCentral upload task that lived here is gone
// as of peat#883. Gradle's `Exec` task swallowed curl's stdout/stderr
// by default, which left no signal in workflow logs when the Sonatype
// Central upload silently dropped a bundle (peat-ffi 0.1.1 incident:
// task reported BUILD SUCCESSFUL in 1.2s, no deployment ever appeared
// on Sonatype). The upload + status-polling logic now lives in
// `.github/workflows/publish-maven.yml`, where stdout/stderr is
// visible inline. This file is responsible only for producing the
// bundle ZIP via the `createMavenCentralBundle` task above; the
// workflow's "Upload to Sonatype Central" step takes it from there.
