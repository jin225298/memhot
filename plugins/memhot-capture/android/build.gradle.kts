import java.io.File
import java.net.URI
import java.security.MessageDigest
import java.util.zip.ZipFile

plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

// Published upstream artifacts, pinned by SHA-256. Nothing is downloaded at app startup.
val speechDir = layout.buildDirectory.dir("speech-runtime")
val speechRuntime = tasks.register("prepareSpeechRuntime") {
    val targetRoot = speechDir.get().asFile
    val assets = listOf(
        Triple("sherpa-onnx.aar", "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/sherpa-onnx-1.13.8.aar", "633c24321e06b1fe79feafa03ea16cbc0f8a286641e2da3559bac91bdb13bd96"),
        Triple("assets/silero_vad.onnx", "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx", "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6"),
    )
    outputs.files(assets.map { speechDir.get().file(it.first) })
    outputs.file(speechDir.map { it.file("classes.jar") })
    outputs.dir(speechDir.map { it.dir("jni") })
    doLast {
        for ((name, url, expected) in assets) {
            val target = File(targetRoot, name)
            fun checksum(file: File): String = MessageDigest.getInstance("SHA-256").let { md ->
                file.inputStream().use { input -> val buffer = ByteArray(65536); while (true) { val n = input.read(buffer); if (n < 0) break; md.update(buffer, 0, n) } }
                md.digest().joinToString("") { "%02x".format(it) }
            }
            if (target.isFile && checksum(target) == expected) continue
            target.parentFile.mkdirs()
            val part = File(target.path + ".part")
            try {
                URI(url).toURL().openConnection().apply { connectTimeout = 30000; readTimeout = 120000 }
                    .getInputStream().use { input -> part.outputStream().use { input.copyTo(it) } }
                check(checksum(part) == expected) { "Speech runtime checksum mismatch: $name" }
                check(part.renameTo(target)) { "Cannot install speech runtime: $name" }
            } finally { part.delete() }
        }
        // A library plugin cannot embed a local AAR directly; retain its classes and JNI ABI layout.
        ZipFile(File(targetRoot, "sherpa-onnx.aar")).use { zip ->
            zip.entries().asSequence().filter { !it.isDirectory && (it.name == "classes.jar" || it.name.matches(Regex("jni/[^/]+/[^/]+\\.so"))) }.forEach { entry ->
                val target = File(targetRoot, entry.name)
                target.parentFile.mkdirs()
                zip.getInputStream(entry).use { input -> target.outputStream().use { input.copyTo(it) } }
            }
        }
    }
}

extensions.configure<com.android.build.api.dsl.LibraryExtension>("android") {
    namespace = "com.angelkang.memhot.capture"
    compileSdk = 36
    defaultConfig {
        minSdk = 24
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        consumerProguardFiles("consumer-rules.pro")
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    sourceSets.getByName("main").assets.directories.add(speechDir.get().dir("assets").asFile.path)
    sourceSets.getByName("main").jniLibs.directories.add(speechDir.get().dir("jni").asFile.path)
    androidResources { noCompress += "onnx" }
}
tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>().configureEach {
    compilerOptions.jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_1_8)
}

tasks.named("preBuild").configure { dependsOn(speechRuntime) }

dependencies {
    implementation(files(speechDir.map { it.file("classes.jar") }).builtBy(speechRuntime))
    implementation(project(":tauri-android"))
    implementation("androidx.core:core-ktx:1.9.0")
    implementation("org.apache.commons:commons-compress:1.28.0")
    implementation("androidx.work:work-runtime:2.11.2")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    // Same mapper as Tauri's Invoke constructor; test-only, already present at runtime.
    androidTestImplementation("com.fasterxml.jackson.core:jackson-databind:2.15.3")
}
