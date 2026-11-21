import org.jetbrains.intellij.platform.gradle.IntelliJPlatformType
import org.jetbrains.intellij.platform.gradle.TestFrameworkType
import org.jetbrains.intellij.platform.gradle.argumentProviders.IntelliJPlatformArgumentProvider
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
        // The JSON plugin, for the schema it gives `newspeak.json`. plugin.xml
        // declares the dependency optional, as the spellchecker's.
        bundledPlugin("com.intellij.modules.json")
    }
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.opentest4j:opentest4j:1.3.0")
}

val repositoryRoot = layout.projectDirectory.dir("../..").asFile

// The project file's schema has one copy, at the repository root, which
// ns-next's reader is held to; the plugin bundles that copy.
tasks.processResources {
    from(repositoryRoot.resolve("schemas/newspeak.schema.json")) {
        into("schemas")
    }
}

val executableSuffix = if (System.getProperty("os.name").startsWith("Windows")) ".exe" else ""

/** The debug build is what the tests use: the release profile's fat LTO makes that build slow. */
val buildLanguageServerDebug = tasks.register<Exec>("buildLanguageServerDebug") {
    description = "Builds ns-next-analyzer (debug) with Cargo for the tests."
    workingDir = repositoryRoot
    commandLine("cargo", "build", "-p", "ns-analyzer")
}

/**
 * Declares as inputs of a test task the `.ns` files under `directories`,
 * which its tests walk through the `nsNext.repositoryRoot` system property:
 * Gradle sees a system property only as a string, so without them an edited
 * source would leave the task up to date. Only the `.ns` files: a golden's
 * recorded outcome (`.toml`) is nothing a plugin test reads.
 */
fun Test.readsNewspeakSources(vararg directories: String) {
    for (directory in directories) {
        inputs.files(fileTree(repositoryRoot.resolve(directory)) { include("**/*.ns") })
            .withPropertyName("sources:$directory")
    }
}

/**
 * Declares as inputs of a test task the Rust sources and build settings an
 * answer of a workspace binary the task starts derives from, in place of the
 * binary itself. The binary is relinked whenever anything it links changes
 * -- `ns-runtime` on nearly every commit, and the gates' own builds of the
 * workspace relink it with every run -- so as an input it would rerun the
 * task after every runtime change, whether or not the answers its tests read
 * could change. The task still depends on the binary's build, which fails
 * the check when the workspace does not compile; what it reads of the
 * binary is named here instead, as file trees, never as anything computed
 * by running a program. A task that starts a binary declares, beside the
 * sources of the code its answers run through, `binaryBuildSettings`.
 */
fun Test.readsRustSources(propertyName: String, sources: FileCollection) {
    inputs.files(sources).withPropertyName(propertyName)
}

/**
 * What every build of a workspace binary depends on beyond the sources of
 * the code it runs: the toolchain, the target's compiler flags, the locked
 * dependency versions, and every member's manifest, since Cargo unifies a
 * dependency's features over the whole graph the binary is built in, and a
 * member may gain a build script without its manifest changing.
 */
val binaryBuildSettings: FileCollection = files(
    repositoryRoot.resolve("Cargo.toml"),
    repositoryRoot.resolve("Cargo.lock"),
    repositoryRoot.resolve("rust-toolchain.toml"),
    repositoryRoot.resolve(".cargo/config.toml"),
    fileTree(repositoryRoot.resolve("crates")) { include("*/Cargo.toml", "*/build.rs") },
)

/** The tests that start a workspace binary, each run by a task of its own with inputs of its own. */
val LEXER_COMPARISON_TEST = "io.github.bstrauss.newspeak.lexer.LexerComparisonTest"
val DAP_SESSION_TEST = "io.github.bstrauss.newspeak.debugger.DapSessionTest"
val DEBUG_PROCESS_TEST = "io.github.bstrauss.newspeak.debugger.NewspeakDebugProcessTest"
val BREAKPOINT_TYPES_TEST = "io.github.bstrauss.newspeak.debugger.NewspeakBreakpointTypesTest"
val TRACED_RUN_END_TO_END_TEST = "io.github.bstrauss.newspeak.devtool.TracedRunEndToEndTest"

tasks.test {
    description = "Runs the plugin's tests that start no workspace binary."
    systemProperty("nsNext.repositoryRoot", repositoryRoot.absolutePath)
    // See `lexerComparisonTest`, `dapSessionTest`, `debugProcessTest`, `breakpointTypesTest` and `traceFileTest`.
    filter {
        excludeTestsMatching(LEXER_COMPARISON_TEST)
        excludeTestsMatching(DAP_SESSION_TEST)
        excludeTestsMatching(DEBUG_PROCESS_TEST)
        excludeTestsMatching(BREAKPOINT_TYPES_TEST)
        excludeTestsMatching(TRACED_RUN_END_TO_END_TEST)
    }
    // The directories `NewspeakDocCommentAgreementTest`, `LexerRestartTest`
    // and `NewspeakHighlightingLexerTest` walk.
    readsNewspeakSources("goldens", "crates/ns-runtime/core-library", "benchmarks")
}

/**
 * Configures a test task of its own to run as `test` runs: on the
 * classpath, JVM and sandbox the platform plugin gives `test`, and after
 * it, since the two share that sandbox. The copy mirrors what version
 * 2.19.0 of the IntelliJ Platform Gradle plugin (`plugins` above)
 * configures `test` with (its `TestIdeTask` configuration and
 * `PrepareTestTask`): an upgrade re-checks it. `testClass` is the one
 * class the task runs, which `test` leaves out.
 */
fun Test.runsLikeUnitTests(testClass: String) {
    group = "verification"
    val unitTests = tasks.test.get()
    testClassesDirs = unitTests.testClassesDirs
    classpath = unitTests.classpath
    javaLauncher = unitTests.javaLauncher
    // The platform's provider reads the heap settings off the task it was
    // made for, and a task naming another task cannot be kept in the
    // configuration cache, so it is made again for this one.
    jvmArgumentProviders.addAll(
        unitTests.jvmArgumentProviders.map { provider ->
            if (provider is IntelliJPlatformArgumentProvider) {
                IntelliJPlatformArgumentProvider(
                    provider.intellijPlatformConfiguration,
                    provider.coroutinesJavaAgentFile,
                    provider.runtimeArchProvider,
                    this,
                )
            } else {
                provider
            }
        },
    )
    systemProperties(unitTests.systemProperties)
    enableAssertions = unitTests.enableAssertions
    setDependsOn(unitTests.dependsOn)
    mustRunAfter(unitTests)
    filter { includeTestsMatching(testClass) }
}

/**
 * The lexer comparison, the one test that starts `ns-next-analyzer`, as a
 * task of its own: its inputs are what `dump-tokens` answers derive from,
 * so a runtime change leaves it up to date while a change to the Rust
 * lexer reruns it.
 */
val lexerComparisonTest = tasks.register<Test>("lexerComparisonTest") {
    description = "Compares the Kotlin lexer with ns-syntax's, through ns-next-analyzer dump-tokens."
    runsLikeUnitTests(LEXER_COMPARISON_TEST)

    dependsOn(buildLanguageServerDebug)
    systemProperty(
        "nsNext.languageServer",
        repositoryRoot.resolve("target/debug/ns-next-analyzer$executableSuffix").absolutePath,
    )
    // `dump-tokens` runs `main` -- its processor check, which records the
    // platform's reach -- and the command's dispatch, then lexes each file
    // with `ns-syntax`'s lexer and prints its tokens, `TokenKind`'s names
    // included, and `ns-syntax`'s length check; nothing of `ns-runtime`
    // past `reach`, nor of the server.
    readsRustSources(
        "dumpTokensSources",
        files(
            fileTree(repositoryRoot.resolve("crates/ns-syntax/src")),
            listOf(
                "crates/ns-analyzer/src/main.rs",
                "crates/ns-analyzer/src/lib.rs",
                "crates/ns-analyzer/src/commands/mod.rs",
                "crates/ns-analyzer/src/commands/dump_tokens.rs",
                "crates/ns-analyzer/src/token_dump.rs",
                "crates/ns-tooling/src/lib.rs",
                "crates/ns-tooling/src/processor.rs",
                "crates/ns-memory/src/reach.rs",
            ).map { path -> repositoryRoot.resolve(path) },
        ),
    )
    readsRustSources("binaryBuildSettings", binaryBuildSettings)
    // The directories `LexerComparisonTest` walks.
    readsNewspeakSources("corpus/modules", "goldens", "crates/ns-runtime/core-library", "benchmarks")
}

/** The runtime the DAP session test debugs, a debug build for the reason the server's is. */
val buildRuntimeDebug = tasks.register<Exec>("buildRuntimeDebug") {
    description = "Builds ns-next (debug) with Cargo for the tests."
    workingDir = repositoryRoot
    commandLine("cargo", "build", "-p", "ns-cli")
}

/**
 * Gives a test task the debug build of `ns-next`, named by the system
 * property `nsNext.runtime`, and declares as its inputs what the answers of
 * `ns-next run --debug-connect` run through: the whole runtime -- the agent
 * in `ns-cli`, the debugging API, the VM and the shipped modules -- so every
 * change to the runtime's sources reruns it, while a change to the analyzer
 * alone leaves it up to date.
 */
fun Test.debugsWithTheRuntime() {
    dependsOn(buildRuntimeDebug)
    systemProperty(
        "nsNext.runtime",
        repositoryRoot.resolve("target/debug/ns-next$executableSuffix").absolutePath,
    )
    readsRustSources(
        "runtimeSources",
        files(
            listOf(
                "crates/ns-cli/src",
                "crates/ns-runtime/src",
                "crates/ns-runtime/core-library",
                "crates/ns-tooling/src",
                "crates/ns-syntax/src",
                "crates/ns-memory/src",
                "crates/ns-assembler/src",
            ).map { directory -> fileTree(repositoryRoot.resolve(directory)) },
        ),
    )
    readsRustSources("binaryBuildSettings", binaryBuildSettings)
}

/** The DAP client against the real agent, as a task of its own. */
val dapSessionTest = tasks.register<Test>("dapSessionTest") {
    description = "Debugs a program with ns-next through the plugin's DAP client."
    runsLikeUnitTests(DAP_SESSION_TEST)
    debugsWithTheRuntime()
}

/** The platform's debugger over the debug process, against the real agent, as a task of its own. */
val debugProcessTest = tasks.register<Test>("debugProcessTest") {
    description = "Debugs a Newspeak run configuration with ns-next through the platform's debugger."
    runsLikeUnitTests(DEBUG_PROCESS_TEST)
    debugsWithTheRuntime()
    // The test tasks share one sandbox, which `runsLikeUnitTests` orders after `test` only.
    mustRunAfter(lexerComparisonTest, dapSessionTest)
}

/**
 * The breakpoint types, as a task of their own: one of their tests stops
 * at a temporary breakpoint in the real runtime, the others need nothing
 * beyond the platform.
 */
val breakpointTypesTest = tasks.register<Test>("breakpointTypesTest") {
    description = "Sets line, method and exception breakpoints, and stops ns-next at a temporary one."
    runsLikeUnitTests(BREAKPOINT_TYPES_TEST)
    debugsWithTheRuntime()
    mustRunAfter(lexerComparisonTest, dapSessionTest, debugProcessTest)
}

/**
 * A traced run end to end, as a task of its own: Run with the Developer
 * Tool's options against the real runtime, its trace file read into the
 * tool and the editor decorated from it.
 */
val traceFileTest = tasks.register<Test>("traceFileTest") {
    description = "Runs a traced Newspeak run configuration with ns-next and reads its trace file into the developer tool."
    runsLikeUnitTests(TRACED_RUN_END_TO_END_TEST)
    debugsWithTheRuntime()
    mustRunAfter(lexerComparisonTest, dapSessionTest, debugProcessTest, breakpointTypesTest)
}

tasks.check {
    dependsOn(lexerComparisonTest, dapSessionTest, debugProcessTest, breakpointTypesTest, traceFileTest)
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

val buildLanguageServerRelease = tasks.register<Exec>("buildLanguageServerRelease") {
    description = "Builds ns-next-analyzer (release) with Cargo for the plugin distribution."
    workingDir = repositoryRoot
    commandLine("cargo", "build", "-p", "ns-analyzer", "--release")
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
        from(repositoryRoot.resolve("target/release/ns-next-analyzer$executableSuffix")) {
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
