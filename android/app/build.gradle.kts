import java.text.SimpleDateFormat
import java.util.Date

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// The hunter is Rust (core/, the same as Mac and Windows): android/rust/ built for the phone
// (libhunt.so, with cargo-ndk) and its Kotlin made by uniffi-bindgen, before every build.
// Needs rustup's toolchain with the aarch64-linux-android target, and cargo-ndk.
val repo = rootProject.file("..")
val rustLibs = layout.buildDirectory.dir("rust/jniLibs")
val rustKotlin = layout.buildDirectory.dir("rust/kotlin")
val home = System.getProperty("user.home")
val cargoPath = listOf("$home/.cargo/bin", "/opt/homebrew/opt/rustup/bin", System.getenv("PATH") ?: "").joinToString(":")

android {
    namespace = "com.pokehunt"
    compileSdk = 35
    ndkVersion = "27.2.12479018"

    defaultConfig {
        applicationId = "com.pokehunt"
        minSdk = 30  // screenshots from an accessibility service
        targetSdk = 35
        // every build is newer than the last, and says when it was built
        // (shown at the bottom of the app), so it's clear which one a phone has
        versionCode = (System.currentTimeMillis() / 60000 - 29_000_000).toInt()
        versionName = SimpleDateFormat("d MMM HH:mm").format(Date())
        // every phone of the last years: half the download of all four kinds
        ndk { abiFilters += "arm64-v8a" }
    }
    buildFeatures { buildConfig = true }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("debug")  // installable as is
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    testOptions {
        unitTests.isReturnDefaultValues = true  // android.util.Log in tests
    }
    sourceSets["main"].java.srcDir(rustKotlin)
    sourceSets["main"].jniLibs.srcDir(rustLibs)
}

val buildRust by tasks.registering(Exec::class) {
    description = "libhunt.so: the hunter, for arm64 phones"
    workingDir = repo
    environment("PATH", cargoPath)
    environment("ANDROID_NDK_HOME", android.ndkDirectory.absolutePath)
    commandLine("cargo", "ndk", "-t", "arm64-v8a", "--platform", "30", "-o", rustLibs.get().asFile.absolutePath,
                "build", "--release", "-p", "hunt-android")
}

val rustHost by tasks.registering(Exec::class) {
    description = "libhunt for this computer: what the Kotlin is made from (the phone's is stripped)"
    workingDir = repo
    environment("PATH", cargoPath)
    commandLine("cargo", "build", "-q", "-p", "hunt-android", "-p", "uniffi-bindgen")
}

val rustBindings by tasks.registering(Exec::class) {
    description = "The Kotlin for libhunt.so (com.pokehunt.core)"
    dependsOn(buildRust, rustHost)
    workingDir = repo
    environment("PATH", cargoPath)
    val lib = if (System.getProperty("os.name").startsWith("Mac")) "libhunt.dylib" else "libhunt.so"
    commandLine("target/debug/uniffi-bindgen", "generate",
                "--library", "target/debug/$lib", "--language", "kotlin",
                "--config", "android/rust/uniffi.toml", "--no-format", "--out-dir", rustKotlin.get().asFile.absolutePath)
}

tasks.named("preBuild") { dependsOn(rustBindings) }

tasks.withType<Test>().configureEach {
    systemProperty("jna.library.path", repo.resolve("target/debug").absolutePath)
}

dependencies {
    implementation("androidx.core:core:1.13.1")
    implementation("com.google.mlkit:text-recognition:16.0.1")  // bundled: works offline, no Play Services needed
    implementation("net.java.dev.jna:jna:5.17.0@aar")  // how the Kotlin reaches libhunt.so
    // the tests run the hunter on this computer (target/debug's libhunt), through the same Kotlin
    testImplementation("junit:junit:4.13.2")
    testImplementation("net.java.dev.jna:jna:5.17.0")
}
