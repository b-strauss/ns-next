rootProject.name = "newspeak-jetbrains"

plugins {
    // Provisions the JDK 21 toolchain the build asks for, so the build does
    // not depend on which JDK happens to run Gradle.
    id("org.gradle.toolchains.foojay-resolver-convention") version "1.0.0"
}
