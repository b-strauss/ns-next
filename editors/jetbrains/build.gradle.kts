import org.jetbrains.intellij.platform.gradle.IntelliJPlatformType
import org.jetbrains.intellij.platform.gradle.TestFrameworkType
import org.jetbrains.intellij.platform.gradle.tasks.PrepareSandboxTask
import java.util.Properties

plugins {
    id("org.jetbrains.kotlin.jvm") version "2.4.10"
    id("org.jetbrains.intellij.platform") version "2.19.0"
}

group = "io.github.bstrauss.newspeak"
version = providers.gradleProperty("pluginVersion").get()

kotlin {
    jvmToolchain(21)
}

repositories {
    mavenCentral()
    intellijPlatform {
        defaultRepositories()
    }
}

dependencies {
    intellijPlatform {
        intellijIdea(providers.gradleProperty("platformVersion"))
        testFramework(TestFrameworkType.Platform)
        // The spellchecker is a content module of the platform itself, not a
        // separate bundled plugin (no plugins/*/plugin.xml of its own -- its
        // descriptor lives inside lib/intellij.spellchecker.jar, and its
        // module id is a `pluginAlias` in product-info.json). plugin.xml
        // still declares an optional `<depends>` on it -- the platform's own
        // extenders of `spellchecker.support` do the same -- so an IDE built
        // without the module still loads this plugin; this entry is only
        // what puts its classes on the compile and test classpath.
        bundledModule("intellij.spellchecker")
    }
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.opentest4j:opentest4j:1.3.0")
}

val repositoryRoot = layout.projectDirectory.dir("../..").asFile

val executableSuffix = if (System.getProperty("os.name").startsWith("Windows")) ".exe" else ""

/** The debug build is what the tests use: the release profile's fat LTO makes that build slow. */
val buildLanguageServerDebug by tasks.registering(Exec::class) {
    description = "Builds ns-next-lsp (debug) with Cargo for the tests."
    workingDir = repositoryRoot
    commandLine("cargo", "build", "-p", "ns-lsp")
}

tasks.test {
    dependsOn(buildLanguageServerDebug)
    systemProperty("nsNext.repositoryRoot", repositoryRoot.absolutePath)
    systemProperty(
        "nsNext.languageServer",
        repositoryRoot.resolve("target/debug/ns-next-lsp$executableSuffix").absolutePath,
    )
    // The tests read these through the system properties above, which Gradle
    // sees only as strings. Declared as inputs, a change to the Rust lexer
    // (through the rebuilt server), the kernel, a corpus module or a golden
    // reruns the lexer comparison instead of leaving `check` up to date.
    inputs.file(repositoryRoot.resolve("target/debug/ns-next-lsp$executableSuffix"))
        .withPropertyName("languageServer")
    inputs.dir(repositoryRoot.resolve("corpus/modules")).withPropertyName("corpusModules")
    inputs.dir(repositoryRoot.resolve("goldens")).withPropertyName("goldens")
    inputs.file(repositoryRoot.resolve("crates/ns-runtime/core-library/Kernel.ns"))
        .withPropertyName("kernel")
}

tasks.withType<PrepareSandboxTask>().configureEach {
    // The full IntelliJ IDEA Ultimate distribution bundles a closed-source,
    // obfuscated "ultimate" marker plugin whose postStartupActivity cannot be
    // instantiated headlessly (it needs a live license); disabling it in the
    // sandbox is the platform's own answer to that, not a workaround for
    // anything this plugin does.
    disabledPlugins.add("com.intellij.modules.ultimate")
}

/**
 * The directory under `bin/` for the platform this build runs on -- only its
 * server is bundled for now -- from the mapping `ServerBinaryLocator` finds
 * the server through at run time, looked up by the rule that file's header
 * states: the system by what `os.name` starts with, the architecture by
 * `os.arch` in lower case. `platformDirectory` follows the same rule; this
 * script's code cannot be called from a test, so the stated rule is what
 * keeps the two lookups one. A host the mapping does not name fails the
 * build when a sandbox is prepared rather than bundling the server under a
 * directory the plugin would never look in.
 */
val hostPlatform: Provider<String> = providers
    .fileContents(layout.projectDirectory.file(
        "src/main/resources/io/github/bstrauss/newspeak/lsp/platform-directories.properties",
    ))
    .asText
    .map { text ->
        val mapping = Properties().apply { load(text.reader()) }
        val operatingSystemName = System.getProperty("os.name")
        val operatingSystemArchitecture = System.getProperty("os.arch")
        val system = mapping.stringPropertyNames()
            .filter { key -> key.startsWith("system.") }
            .firstOrNull { key -> operatingSystemName.startsWith(key.removePrefix("system.")) }
            ?.let(mapping::getProperty)
        val architecture = mapping.getProperty("architecture.${operatingSystemArchitecture.lowercase()}")
        if (system == null || architecture == null) {
            throw GradleException(
                "platform-directories.properties names no bin/ directory for this host " +
                    "(os.name \"$operatingSystemName\", os.arch \"$operatingSystemArchitecture\"), " +
                    "and the plugin would not find a server bundled under any other",
            )
        }
        "$system-$architecture"
    }

val buildLanguageServerRelease by tasks.registering(Exec::class) {
    description = "Builds ns-next-lsp (release) with Cargo for the plugin distribution."
    workingDir = repositoryRoot
    commandLine("cargo", "build", "-p", "ns-lsp", "--release")
}

// Every sandbox an IDE is started from gets the server: `prepareSandbox`, which
// `buildPlugin` packages, and the separate `prepareSandbox_runIde` (and its
// split-mode siblings), which `runIde` starts from. Configuring `prepareSandbox`
// alone left `runIde`'s sandbox without the server. The test sandboxes are left
// out: the tests use the debug build, and the release build is slow.
tasks.withType<PrepareSandboxTask>()
    .matching { task -> !task.name.startsWith("prepareTest") }
    .configureEach {
        dependsOn(buildLanguageServerRelease)
        // Beside the jars rather than inside one, so it starts without being extracted.
        from(repositoryRoot.resolve("target/release/ns-next-lsp$executableSuffix")) {
            into(pluginName.zip(hostPlatform) { name, platform -> "$name/bin/$platform" })
        }
        from(layout.projectDirectory.file("THIRD-PARTY-NOTICES.txt")) {
            into(pluginName)
        }
        from(repositoryRoot.resolve("LICENSE")) {
            into(pluginName)
        }
    }

intellijPlatform {
    pluginConfiguration {
        id = "io.github.bstrauss.newspeak"
        name = "Newspeak (ns-next)"
        version = providers.gradleProperty("pluginVersion")
        ideaVersion {
            // IntelliJ IDEA 2026.1.4, build 261.26222.65: the first release
            // with the reworked LSP API this plugin uses exclusively.
            sinceBuild = "261.26222"
            untilBuild = provider { null }
        }
        vendor {
            name = "Benjamin Strauß"
            url = "https://github.com/b-strauss/ns-next"
        }
    }
    signing {
        certificateChain = providers.environmentVariable("NEWSPEAK_PLUGIN_CERTIFICATE_CHAIN")
        privateKey = providers.environmentVariable("NEWSPEAK_PLUGIN_PRIVATE_KEY")
        password = providers.environmentVariable("NEWSPEAK_PLUGIN_PRIVATE_KEY_PASSWORD")
    }
    publishing {
        token = providers.environmentVariable("NEWSPEAK_PLUGIN_PUBLISH_TOKEN")
    }
    pluginVerification {
        ides {
            // The lowest supported and the current release of four commercial
            // IDEs: enough to catch an API one of them lacks, without
            // downloading every patch release. Re-derive these versions with
            // `./editors/jetbrains/gradlew -p editors/jetbrains
            // printProductsReleases`; widen the product types below first
            // (add or change a `create(IntelliJPlatformType.<type>, ...)`
            // line) to have it also print releases for a newly added IDE.
            create(IntelliJPlatformType.IntellijIdea, "2026.1.4")
            create(IntelliJPlatformType.IntellijIdea, providers.gradleProperty("platformVersion"))
            create(IntelliJPlatformType.RustRover, "2026.1.4")
            create(IntelliJPlatformType.RustRover, "2026.2.3")
            create(IntelliJPlatformType.PyCharm, "2026.1.5")
            create(IntelliJPlatformType.CLion, "2026.2.3")
        }
    }
}
