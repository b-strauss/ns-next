# Newspeak (ns-next) — JetBrains plugin

Syntax highlighting, diagnostics and semantic tokens for `.ns` files, through
the `ns-next-lsp` language server from `crates/ns-lsp`. The design, its
components and the platform version it targets are in
`docs/design/editor-support.md`.

## Opening it in IntelliJ IDEA

Open this directory (`editors/jetbrains`) as a project; IntelliJ imports it
through the Gradle wrapper. Any JDK from 17 to 26 can run Gradle (*Settings →
Build Tools → Gradle → Gradle JVM*); the JDK 21 the build compiles with is
provisioned by the build itself. The first sync downloads the target IDE into
`.intellijPlatform/`. The *Plugin DevKit* plugin helps with `plugin.xml`.

Opened from the repository root instead, link the build through the Gradle
tool window's **+** and pick `editors/jetbrains/build.gradle.kts`.

## Commands

From this directory (`gradlew.bat` on Windows outside Git Bash):

```bash
# a sandbox IDE with the plugin and a release build of ns-next-lsp
./gradlew runIde

# the tests, including LexerComparisonTest against the Rust lexer
# (builds ns-next-lsp in debug first)
./gradlew check

# the distributable archive: build/distributions/newspeak-jetbrains-<version>.zip
./gradlew buildPlugin

# the plugin verifier against the IDEs listed in build.gradle.kts
# (downloads several IDEs)
./gradlew verifyPlugin
```

Every task that prepares a sandbox builds `ns-next-lsp` with Cargo itself, so
`cargo` must be on `PATH`; the binary is bundled for the platform the build
runs on only.

A change to `../../crates/ns-syntax/src/lexer.rs` must be mirrored in
`src/main/kotlin/io/github/bstrauss/newspeak/lexer/NewspeakLexer.kt` in the
same commit, or `check` fails.
