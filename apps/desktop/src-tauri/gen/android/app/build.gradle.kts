import java.io.File
import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

// Release signing: `gen/android/keystore.properties` (gitignored, never
// committed; CI writes it from the secrets – see "Build, CI and signing"
// under "Mobile (Android)" in docs/ARCHITECTURE.md) with `storeFile` (the
// keystore, outside the repository – an absolute path is best; a relative
// one counts from gen/android), `password` (keystore, and the key unless
// `keyPassword` is set) and `keyAlias`. Without the file a release build
// is signed with the debug key, so local release builds still install –
// such an APK can never update an app signed with the release key.
val keystoreProperties = Properties().apply {
    val propFile = rootProject.file("keystore.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}
val releaseStoreFile: File? =
    keystoreProperties.getProperty("storeFile")?.let { rootProject.file(it).canonicalFile }
val hasReleaseKeystore = releaseStoreFile != null
// The repository is public: a keystore inside the checkout (the nearest
// folder above gen/android with a `.git`) is refused, not merely
// gitignored – one `git add -A` would publish the key that can never be
// replaced for installed apps.
releaseStoreFile?.let { store ->
    val repository = generateSequence(rootProject.rootDir.canonicalFile) { it.parentFile }
        .firstOrNull { File(it, ".git").exists() }
    if (repository != null && store.startsWith(repository)) {
        throw GradleException(
            "keystore.properties: storeFile must lie outside the repository ($repository)"
        )
    }
}

android {
    compileSdk = 37
    namespace = "io.github.cedrickgd.keystead"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "io.github.cedrickgd.keystead"
        minSdk = 29
        targetSdk = 37
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
    }
    signingConfigs {
        if (hasReleaseKeystore) {
            create("release") {
                storeFile = releaseStoreFile
                storePassword = keystoreProperties.getProperty("password")
                keyAlias = keystoreProperties.getProperty("keyAlias")
                keyPassword = keystoreProperties.getProperty("keyPassword")
                    ?: keystoreProperties.getProperty("password")
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            signingConfig = signingConfigs.getByName(
                if (hasReleaseKeystore) "release" else "debug"
            )
            optimization {
               enable = true
            }
            proguardFiles(
                *fileTree(".") {
                  include("**/*.pro")
                  exclude("build/**")
                }.files.toTypedArray()
            )
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    buildFeatures {
        buildConfig = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = file("tauri.build.gradle.kts"))
