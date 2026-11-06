import com.android.build.api.variant.ApplicationVariant
import javax.inject.Inject

// AGP 9 compiles Kotlin itself, so there is no org.jetbrains.kotlin.android plugin here.
plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.compose.compiler)
}

android {
    namespace = "app.homespace.android"
    compileSdk = libs.versions.android.compileSdk.get().toInt()

    defaultConfig {
        applicationId = "app.homespace.android"
        minSdk = libs.versions.android.minSdk.get().toInt()
        targetSdk = libs.versions.android.targetSdk.get().toInt()
        // The release workflow passes these from the git tag; local builds keep the defaults.
        versionCode = providers.gradleProperty("homespace.versionCode").orNull?.toInt() ?: 1
        versionName = providers.gradleProperty("homespace.versionName").orNull ?: "0.1.0"
    }

    buildFeatures {
        compose = true
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles("proguard-rules.pro")
        }
    }
}

// The client is Rust (clients/mobile-core), called through UniFFI-generated Kotlin bindings.

/** Runs `mobile-core/scripts/build-android.sh`, rebuilding when any client crate changes. */
abstract class RustCoreTask @Inject constructor(private val exec: ExecOperations) : DefaultTask() {
    @get:Input abstract val mode: Property<String>
    @get:Input abstract val profile: Property<String>
    @get:Input abstract val androidHome: Property<String>
    @get:InputFiles @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val sources: ConfigurableFileCollection
    @get:Internal abstract val script: RegularFileProperty
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun build() {
        exec.exec {
            environment("ANDROID_HOME", androidHome.get())
            commandLine(
                script.get().asFile.path, mode.get(), outputDir.get().asFile.path, profile.get(),
            )
        }
    }
}

val clientsDir = rootProject.layout.projectDirectory.dir("..")
val rustSources = files(
    listOf("mobile-core", "api-client", "client-core", "../shared/protocol").map { crate ->
        clientsDir.dir(crate).asFileTree.matching {
            include("src/**", "Cargo.toml")
        }
    },
    clientsDir.file("Cargo.lock"),
)

fun RustCoreTask.configure(mode: String, profile: String, out: String) {
    this.mode = mode
    this.profile = profile
    androidHome = androidComponents.sdkComponents.sdkDirectory.map { it.asFile.path }
    sources.from(rustSources)
    script = clientsDir.file("mobile-core/scripts/build-android.sh")
    outputDir = layout.buildDirectory.dir(out)
}

val rustBindings = tasks.register<RustCoreTask>("generateRustCoreBindings") {
    configure("bindings", "debug", "generated/rust-core/kotlin")
}

androidComponents {
    onVariants { variant: ApplicationVariant ->
        val profile = if (variant.buildType == "release") "mobile" else "debug"
        val jni = tasks.register<RustCoreTask>(
            "buildRustCore${variant.name.replaceFirstChar(Char::uppercase)}",
        ) {
            configure("jni", profile, "generated/rust-core/jniLibs/${variant.name}")
        }
        variant.sources.kotlin?.addGeneratedSourceDirectory(rustBindings, RustCoreTask::outputDir)
        variant.sources.jniLibs?.addGeneratedSourceDirectory(jni, RustCoreTask::outputDir)
    }
}

/** The version of the Kotlin half of rustls-platform-verifier that the Rust half expects. */
abstract class RustlsVerifierVersion : ValueSource<String, RustlsVerifierVersion.Params> {
    interface Params : ValueSourceParameters {
        val lockFile: RegularFileProperty
    }

    override fun obtain(): String {
        val lines = parameters.lockFile.get().asFile.readLines()
        val name = lines.indexOf("name = \"rustls-platform-verifier-android\"")
        check(name >= 0) { "rustls-platform-verifier-android not found in Cargo.lock" }
        return lines[name + 1].substringAfter('"').substringBefore('"')
    }
}

val rustlsVerifierVersion = providers.of(RustlsVerifierVersion::class.java) {
    parameters.lockFile = clientsDir.file("Cargo.lock")
}

configurations.configureEach {
    resolutionStrategy.eachDependency {
        if (requested.group == "org.rustls" && requested.name == "rustls-platform-verifier") {
            useVersion(rustlsVerifierVersion.get())
            because("it must be the version the Rust crate was built against")
        }
    }
}

tasks.withType<Test>().configureEach {
    // JVM unit tests load the host build of the library (built by generateRustCoreBindings).
    systemProperty("jna.library.path", clientsDir.dir("target/debug").asFile.path)
    // JNA loads native code; without this, newer JDKs warn on every run.
    jvmArgs("--enable-native-access=ALL-UNNAMED")
}

dependencies {
    // The UniFFI bindings call the library through JNA and await it with coroutines.
    implementation("${libs.jna.get()}@aar")
    implementation(libs.kotlinx.coroutines.core)
    implementation(libs.rustls.platform.verifier)

    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.browser)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.work.runtime)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.tooling.preview)
    debugImplementation(libs.androidx.compose.ui.tooling)

    // The plain jar carries the desktop JNA natives the JVM tests need.
    testImplementation(libs.jna)
    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
}
