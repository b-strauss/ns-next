import org.jetbrains.intellij.platform.gradle.TestFrameworkType
import org.jetbrains.intellij.platform.gradle.tasks.PrepareSandboxTask

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
    }
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.opentest4j:opentest4j:1.3.0")
}

val repositoryRoot = layout.projectDirectory.dir("../..").asFile

tasks.test {
    systemProperty("nsNext.repositoryRoot", repositoryRoot.absolutePath)
}

tasks.withType<PrepareSandboxTask>().configureEach {
    // The full IntelliJ IDEA Ultimate distribution bundles a closed-source,
    // obfuscated "ultimate" marker plugin whose postStartupActivity cannot be
    // instantiated headlessly (it needs a live license); disabling it in the
    // sandbox is the platform's own answer to that, not a workaround for
    // anything this plugin does.
    disabledPlugins.add("com.intellij.modules.ultimate")
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
}
