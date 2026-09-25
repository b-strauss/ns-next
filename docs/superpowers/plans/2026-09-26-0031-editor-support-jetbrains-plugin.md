# Editor Support, Part 2: the JetBrains Plugin — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A JetBrains plugin, `Newspeak (ns-next)`, that gives `.ns` files immediate highlighting from a Kotlin lexer, the editor features that hang on lexer tokens, and — through the bundled `ns-next-lsp` over JetBrains' native LSP API — errors as you type and semantic colours, in every commercial JetBrains IDE from 2026.1.4 on, publishable to the Marketplace at any time.

**Architecture:** A Gradle project under `editors/jetbrains/` (Kotlin, IntelliJ Platform Gradle Plugin 2.19.0) with a hand-written lexer that mirrors `crates/ns-syntax/src/lexer.rs` function by function and is compared against it token by token over every module in the repository, a flat `ParserDefinition` (no grammar), and an `LspIntegrationProvider` that starts `ns-next-lsp` once per project. Gradle builds the server with Cargo and puts it beside the plugin's jars under `bin/<os>-<arch>/`.

**Tech Stack:** Kotlin 2.4.10, Gradle 9.8.0 (wrapper), IntelliJ Platform Gradle Plugin 2.19.0, IntelliJ Platform 2026.2.3 (build target), Java 21 bytecode (toolchain provisioned by the foojay resolver), JUnit 4.13.2.

**Spec:** `docs/design/editor-support.md`. Part 1, `docs/superpowers/plans/2026-09-26-0030-editor-support-language-server.md`, must be complete: this part consumes `ns-next-lsp`, its `dump-tokens` format and its semantic-token legend.

## Global Constraints

- Work in the worktree `D:\Workspace\ns-next-editor` on branch `feature/editor-support`. Never `cd` into `D:\Workspace\ns-next`: another session works there.
- All code, identifiers, comments, commit messages and developer docs in English.
- Names are written out, in Kotlin as in Rust: no identifier of one or two characters and no truncated word. Name every lambda parameter — never Kotlin's implicit `it`.
- Conventional commits. Every commit ends with the `Co-Authored-By:` trailer naming the model that actually wrote the code, as the dispatch instructions give it.
- Plugin ID `io.github.bstrauss.newspeak` (permanent once published), name `Newspeak (ns-next)`, vendor `Benjamin Strauß`, licence MIT, `sinceBuild` `261.26222` (IntelliJ IDEA 2026.1.4 is build 261.26222.65), no `untilBuild`.
- Only the new LSP API: `<depends>com.intellij.modules.lsp</depends>`, extension point `com.intellij.platform.lsp.integrationProvider`, `LspIntegrationProvider`, `ProjectWideLspClientDescriptor`, `LspClientDescriptor.lspCustomization`. Never the deprecated pre-2026.1.4 names (`com.intellij.modules.ultimate`, `serverSupportProvider`, `LspServerSupportProvider`, `LspServerDescriptor`).
- No third-party plugin dependency (no LSP4IJ). No runtime library beyond what the platform provides.
- Nothing machine-specific in the repository: no absolute paths, no signing certificate, no Marketplace token. Signing and publishing read environment variables.
- `editors/jetbrains/build/`, `editors/jetbrains/.gradle/`, `editors/jetbrains/.intellijPlatform/` and `editors/jetbrains/.kotlin/` are ignored.
- Per-commit gate for every task (Git Bash, from the repository root): `./editors/jetbrains/gradlew -p editors/jetbrains check`. A task that also touches Rust runs `cargo fmt --all`, `cargo clippy --workspace --all-targets` and `cargo test -p ns-lsp` too. `verifyPlugin` and `buildPlugin` run once, in Task 7.
- Run every command in the foreground. Do not hand back while a background command runs. The first Gradle run downloads the IntelliJ Platform (several GB) and a JDK 21; that is expected.

## Review Focus

1. **Lexing from the middle of a file.** The editor re-lexes from arbitrary token boundaries with the state it stored there. `a -1` (a negative literal) and `a-1` (a subtraction) depend on the previous significant token, which the lexer's state carries. Pinned by Task 2's restart test.
2. **A file with lexer errors.** `:=`, a stray `é` or `😀`, an unterminated string or comment: the lexer must still cover every character with a token (the platform requires it) and never throw. Pinned in Task 2.
3. **No server for this platform, or a configured path that does not exist.** Highlighting and the editor features keep working; one balloon names the problem and links to the setting; nothing is retried in a loop. Pinned by Task 5's locator tests.
4. **Line endings.** A corpus file read as it is on disk, `\r\n` included, must lex identically in both lexers. Pinned by Task 3's comparison, which reads the files unmodified.
5. **Surrogate pairs.** An emoji in a double-quoted one-character string (`"😀"`) is one code point but two UTF-16 chars; the Kotlin lexer must treat it as the Rust lexer treats one character. Pinned in Task 2.

---

## File structure

```
editors/jetbrains/
  .gitattributes                         line endings of the Gradle wrapper scripts
  .gitignore
  settings.gradle.kts
  build.gradle.kts
  gradle.properties
  gradlew, gradlew.bat, gradle/wrapper/  the Gradle 9.8.0 wrapper (generated, committed)
  THIRD-PARTY-NOTICES.txt                licences of the crates inside ns-next-lsp
  src/main/resources/META-INF/plugin.xml
  src/main/resources/META-INF/pluginIcon.svg, pluginIcon_dark.svg
  src/main/resources/icons/newspeak.svg, newspeak_dark.svg
  src/main/kotlin/io/github/bstrauss/newspeak/
    NewspeakLanguage.kt  NewspeakFileType.kt  NewspeakIcons.kt  NewspeakFile.kt
    lexer/NewspeakTokenTypes.kt  lexer/NewspeakLexer.kt
    highlighting/NewspeakColors.kt  highlighting/NewspeakSyntaxHighlighter.kt
    highlighting/NewspeakColorSettingsPage.kt
    parser/NewspeakParserDefinition.kt
    editor/NewspeakBraceMatcher.kt  editor/NewspeakCommenter.kt
    editor/NewspeakQuoteHandler.kt  editor/NewspeakIndexPatternBuilder.kt
    editor/NewspeakSpellcheckingStrategy.kt
    lsp/ServerBinaryLocator.kt  lsp/NewspeakLspIntegrationProvider.kt
    lsp/NewspeakLspClientDescriptor.kt  lsp/NewspeakSemanticTokens.kt
    lsp/MissingServerNotification.kt  lsp/RestartServerAction.kt
    settings/NewspeakSettings.kt  settings/NewspeakSettingsConfigurable.kt
  src/test/kotlin/io/github/bstrauss/newspeak/
    lexer/NewspeakLexerTest.kt  lexer/LexerRestartTest.kt  lexer/LexerComparisonTest.kt
    NewspeakEditorTest.kt  highlighting/NewspeakColorSettingsPageTest.kt
    lsp/ServerBinaryLocatorTest.kt  lsp/NewspeakSemanticTokensTest.kt
```

---

### Task 1: The Gradle project, the language and the file type

**Files:**
- Create: everything under `editors/jetbrains/` listed above except the `lexer/`, `highlighting/`, `parser/`, `editor/`, `lsp/`, `settings/` sources and their tests.
- Create: `src/test/kotlin/io/github/bstrauss/newspeak/NewspeakEditorTest.kt`

**Interfaces:**
- Produces: `object NewspeakLanguage : Language`, `object NewspeakFileType : LanguageFileType`, `object NewspeakIcons { val FILE: Icon }`, `class NewspeakFile(viewProvider: FileViewProvider) : PsiFileBase`, and the constant `const val PLUGIN_ID = "io.github.bstrauss.newspeak"` in `NewspeakLanguage.kt`.

- [ ] **Step 1: Generate the Gradle wrapper**

Gradle is not installed on the development machine; the wrapper is generated once with a downloaded distribution and then committed. In Git Bash, from the repository root, with `SCRATCH` set to the session's scratchpad directory (the dispatch instructions name it; any temporary directory outside the repository works):

```bash
SCRATCH="<the scratchpad directory>"
curl -fsSL -o "$SCRATCH/gradle-9.8.0-bin.zip" https://services.gradle.org/distributions/gradle-9.8.0-bin.zip
curl -fsSL -o "$SCRATCH/gradle-9.8.0-bin.zip.sha256" https://services.gradle.org/distributions/gradle-9.8.0-bin.zip.sha256
echo "$(cat "$SCRATCH/gradle-9.8.0-bin.zip.sha256")  $SCRATCH/gradle-9.8.0-bin.zip" | sha256sum -c -
unzip -q "$SCRATCH/gradle-9.8.0-bin.zip" -d "$SCRATCH"
mkdir -p editors/jetbrains
cd editors/jetbrains
touch settings.gradle.kts
"$SCRATCH/gradle-9.8.0/bin/gradle" wrapper --gradle-version 9.8.0 --distribution-type bin \
  --gradle-distribution-sha256-sum "$(cat "$SCRATCH/gradle-9.8.0-bin.zip.sha256")"
cd ../..
```

Expected: `sha256sum` prints `OK`; `editors/jetbrains/gradlew`, `gradlew.bat`, `gradle/wrapper/gradle-wrapper.jar` and `gradle/wrapper/gradle-wrapper.properties` exist, the properties naming `gradle-9.8.0-bin.zip` and its `distributionSha256Sum`.

- [ ] **Step 2: Line endings and ignores**

`editors/jetbrains/.gitattributes`:

```
# The Unix wrapper script must keep LF to run; the Windows one keeps CRLF.
gradlew text eol=lf
*.bat text eol=crlf
*.jar binary
```

`editors/jetbrains/.gitignore`:

```
/build/
/.gradle/
/.intellijPlatform/
/.kotlin/
```

- [ ] **Step 3: The build**

`editors/jetbrains/settings.gradle.kts`:

```kotlin
rootProject.name = "newspeak-jetbrains"

plugins {
    // Provisions the JDK 21 toolchain the build asks for, so the build does
    // not depend on which JDK happens to run Gradle.
    id("org.gradle.toolchains.foojay-resolver-convention") version "1.0.0"
}
```

`editors/jetbrains/gradle.properties`:

```properties
pluginVersion = 0.1.0
# The platform the plugin is compiled and tested against.
platformVersion = 2026.2.3
# The platform bundles the Kotlin standard library; bundling a second copy
# into the plugin is what the platform's documentation warns against.
kotlin.stdlib.default.dependency = false
org.gradle.configuration-cache = true
org.gradle.caching = true
```

`editors/jetbrains/build.gradle.kts`:

```kotlin
import org.jetbrains.intellij.platform.gradle.TestFrameworkType

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
```

- [ ] **Step 4: The language, the file type, the icons, the file**

`src/main/kotlin/io/github/bstrauss/newspeak/NewspeakLanguage.kt`:

```kotlin
package io.github.bstrauss.newspeak

import com.intellij.lang.Language

/** The plugin's ID, as `plugin.xml` and the Marketplace know it. Permanent once published. */
const val PLUGIN_ID = "io.github.bstrauss.newspeak"

object NewspeakLanguage : Language("Newspeak") {
    private fun readResolve(): Any = NewspeakLanguage
}
```

`NewspeakFileType.kt`:

```kotlin
package io.github.bstrauss.newspeak

import com.intellij.openapi.fileTypes.LanguageFileType
import javax.swing.Icon

object NewspeakFileType : LanguageFileType(NewspeakLanguage) {
    override fun getName(): String = "Newspeak"
    override fun getDescription(): String = "Newspeak module"
    override fun getDefaultExtension(): String = "ns"
    override fun getIcon(): Icon = NewspeakIcons.FILE
}
```

`NewspeakIcons.kt`:

```kotlin
package io.github.bstrauss.newspeak

import com.intellij.openapi.util.IconLoader
import javax.swing.Icon

object NewspeakIcons {
    /** `newspeak_dark.svg` is picked up for dark themes by the naming convention. */
    @JvmField
    val FILE: Icon = IconLoader.getIcon("/icons/newspeak.svg", NewspeakIcons::class.java)
}
```

`NewspeakFile.kt`:

```kotlin
package io.github.bstrauss.newspeak

import com.intellij.extapi.psi.PsiFileBase
import com.intellij.openapi.fileTypes.FileType
import com.intellij.psi.FileViewProvider

class NewspeakFile(viewProvider: FileViewProvider) : PsiFileBase(viewProvider, NewspeakLanguage) {
    override fun getFileType(): FileType = NewspeakFileType
    override fun toString(): String = "Newspeak file"
}
```

`src/main/resources/icons/newspeak.svg` (16×16; the dark variant `newspeak_dark.svg` uses `#CED0D6` for the stroke and fill):

```xml
<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">
  <rect x="1.5" y="1.5" width="13" height="13" rx="2.5" fill="none" stroke="#6C707E"/>
  <path d="M5 11.5V4.5l6 7V4.5" fill="none" stroke="#6C707E" stroke-width="1.4" stroke-linejoin="round"/>
</svg>
```

`src/main/resources/META-INF/pluginIcon.svg` (40×40, the Marketplace icon; `pluginIcon_dark.svg` the same with `#CED0D6`):

```xml
<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40" viewBox="0 0 40 40">
  <rect x="3" y="3" width="34" height="34" rx="7" fill="none" stroke="#3574F0" stroke-width="2.5"/>
  <path d="M13 29V11l14 18V11" fill="none" stroke="#3574F0" stroke-width="3" stroke-linejoin="round"/>
</svg>
```

- [ ] **Step 5: `plugin.xml`**

`src/main/resources/META-INF/plugin.xml`:

```xml
<idea-plugin>
    <id>io.github.bstrauss.newspeak</id>
    <name>Newspeak (ns-next)</name>
    <vendor url="https://github.com/b-strauss/ns-next">Benjamin Strauß</vendor>

    <description><![CDATA[
    <p>Support for <a href="https://newspeaklanguage.org/">Newspeak</a> <code>.ns</code> modules,
    from the <a href="https://github.com/b-strauss/ns-next">ns-next</a> runtime.</p>
    <ul>
      <li>Syntax highlighting as you type, with brace matching, block comments, TODO markers
          and spell checking in comments and strings.</li>
      <li>Errors as you type: every lexer, parser and resolver error ns-next reports before
          running a module, with ns-next's own message.</li>
      <li>Semantic colouring from ns-next's resolver: parameters, locals, slots, nested classes
          and sends.</li>
    </ul>
    <p>The language server, <code>ns-next-lsp</code>, is bundled for Windows x64. On other
    platforms, set its path under <em>Settings | Languages &amp; Frameworks | Newspeak</em>.</p>
    ]]></description>

    <change-notes><![CDATA[
    <p>0.1.0: highlighting, errors as you type and semantic colouring.</p>
    ]]></change-notes>

    <depends>com.intellij.modules.platform</depends>
    <depends>com.intellij.modules.lsp</depends>

    <extensions defaultExtensionNs="com.intellij">
        <fileType name="Newspeak"
                  implementationClass="io.github.bstrauss.newspeak.NewspeakFileType"
                  fieldName="INSTANCE"
                  language="Newspeak"
                  extensions="ns"/>
    </extensions>
</idea-plugin>
```

Later tasks add their extensions to this `<extensions>` element.

- [ ] **Step 6: Write the failing test**

`src/test/kotlin/io/github/bstrauss/newspeak/NewspeakEditorTest.kt`:

```kotlin
package io.github.bstrauss.newspeak

import com.intellij.openapi.fileTypes.FileTypeManager
import com.intellij.testFramework.fixtures.BasePlatformTestCase

class NewspeakEditorTest : BasePlatformTestCase() {
    fun testNsFilesAreNewspeak() {
        assertSame(NewspeakFileType, FileTypeManager.getInstance().getFileTypeByFileName("Module.ns"))
    }
}
```

- [ ] **Step 7: Run it**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains check`
Expected: the first run downloads the platform and a JDK 21, then `BUILD SUCCESSFUL` with 1 test passing. (There is no "failing first" run here: the test pins the registration Step 5 already wrote; run it once before Step 5 if you want to see it fail.) If Gradle reports that the LSP classes or the `com.intellij.modules.lsp` module cannot be resolved in later tasks, add `bundledModule("intellij.platform.lsp")` inside `intellijPlatform { }` in `dependencies` — the platform's LSP page shows only `intellijIdea(...)`, so try without it first.

- [ ] **Step 8: Commit**

```bash
git add editors/jetbrains
git commit -m "feat: JetBrains plugin project with the Newspeak file type"
```

---

### Task 2: The Kotlin lexer

**Files:**
- Create: `src/main/kotlin/io/github/bstrauss/newspeak/lexer/NewspeakTokenTypes.kt`
- Create: `src/main/kotlin/io/github/bstrauss/newspeak/lexer/NewspeakLexer.kt`
- Create: `src/test/kotlin/io/github/bstrauss/newspeak/lexer/NewspeakLexerTest.kt`
- Create: `src/test/kotlin/io/github/bstrauss/newspeak/lexer/LexerRestartTest.kt`

**Interfaces:**
- Produces:
  ```kotlin
  class NewspeakTokenType(debugName: String, val rustKind: String?) : IElementType(debugName, NewspeakLanguage)
  object NewspeakTokenTypes { IDENTIFIER, RESERVED_WORD, KEYWORD, SETTER_KEYWORD, BINARY_SELECTOR,
      CLASS, PRIVATE, PUBLIC, PROTECTED, LEFT_PARENTHESIS, RIGHT_PARENTHESIS, LEFT_BRACKET,
      RIGHT_BRACKET, LEFT_BRACE, RIGHT_BRACE, BAR, DOUBLE_BAR, CARET, DOT, SEMICOLON, COLON,
      EQUALS, COLON_COLON_EQUALS, POUND, LESS_THAN, GREATER_THAN, MIXIN_OPERATOR, NUMBER,
      STRING, SYMBOL, COMMENT; val RESERVED_WORDS: Set<String> }
  class NewspeakLexer : LexerBase()
  ```
  Whitespace is `com.intellij.psi.TokenType.WHITE_SPACE`, a lexer error `com.intellij.psi.TokenType.BAD_CHARACTER`. `rustKind` is the `ns_syntax::TokenKind` variant name the token corresponds to (`RESERVED_WORD`'s is `"Identifier"`); `null` for `COMMENT` — the Rust lexer drops comments.

The lexer mirrors `crates/ns-syntax/src/lexer.rs`, read it side by side. Where the two differ, and why:

- The Rust lexer drops whitespace and comments; this one emits `WHITE_SPACE` and `COMMENT` tokens, because the platform requires every character to belong to a token. Every comment, nested ones included, is one token, so the lexer can restart at any token boundary.
- Where the Rust lexer reports a diagnostic and emits **no** token — `:=`, and a character that starts no token — this one emits `BAD_CHARACTER` over exactly the characters the diagnostic covers. An unterminated string, comment or symbol is one `STRING`, `COMMENT` or `SYMBOL` token to the end, as in Rust.
- `self`, `super`, `outer`, `true`, `false` and `nil` (`newspeak-spec.pdf` §4.1) are `RESERVED_WORD`, an `Identifier` to the Rust lexer, so the highlighter can colour them.
- Offsets are UTF-16 chars, not bytes. Every decision the Rust lexer makes is on ASCII bytes, so the logic carries over unchanged, except where it steps over one *character*: a stray character and the one-character double-quoted string step over one code point, which is two chars for a surrogate pair.
- The Rust lexer asks whether the previous *token* ends a value (`previous_ends_value`) to tell `a -1` from `a-1`. That is the one piece of context a restart needs, so it is the lexer's state: `1` when the previous significant token (not whitespace, a comment or a bad character — the Rust lexer has no token for any of them) ends a value, `0` otherwise.

- [ ] **Step 1: The token types**

`NewspeakTokenTypes.kt`:

```kotlin
package io.github.bstrauss.newspeak.lexer

import com.intellij.psi.tree.IElementType
import io.github.bstrauss.newspeak.NewspeakLanguage

/**
 * A token of the Newspeak lexer. [rustKind] is the `ns_syntax::TokenKind`
 * variant this token corresponds to -- what the lexer comparison checks --
 * or null for a token the Rust lexer does not produce.
 */
class NewspeakTokenType(debugName: String, val rustKind: String?) : IElementType(debugName, NewspeakLanguage)

object NewspeakTokenTypes {
    private fun mirrored(kind: String) = NewspeakTokenType(kind, kind)

    @JvmField val IDENTIFIER = mirrored("Identifier")
    /** An identifier the specification reserves (§4.1); the Rust lexer calls it an `Identifier`. */
    @JvmField val RESERVED_WORD = NewspeakTokenType("ReservedWord", "Identifier")
    @JvmField val KEYWORD = mirrored("Keyword")
    @JvmField val SETTER_KEYWORD = mirrored("SetterKeyword")
    @JvmField val BINARY_SELECTOR = mirrored("BinarySelector")
    @JvmField val CLASS = mirrored("Class")
    @JvmField val PRIVATE = mirrored("Private")
    @JvmField val PUBLIC = mirrored("Public")
    @JvmField val PROTECTED = mirrored("Protected")
    @JvmField val LEFT_PARENTHESIS = mirrored("LeftParenthesis")
    @JvmField val RIGHT_PARENTHESIS = mirrored("RightParenthesis")
    @JvmField val LEFT_BRACKET = mirrored("LeftBracket")
    @JvmField val RIGHT_BRACKET = mirrored("RightBracket")
    @JvmField val LEFT_BRACE = mirrored("LeftBrace")
    @JvmField val RIGHT_BRACE = mirrored("RightBrace")
    @JvmField val BAR = mirrored("Bar")
    @JvmField val DOUBLE_BAR = mirrored("DoubleBar")
    @JvmField val CARET = mirrored("Caret")
    @JvmField val DOT = mirrored("Dot")
    @JvmField val SEMICOLON = mirrored("Semicolon")
    @JvmField val COLON = mirrored("Colon")
    @JvmField val EQUALS = mirrored("Equals")
    @JvmField val COLON_COLON_EQUALS = mirrored("ColonColonEquals")
    @JvmField val POUND = mirrored("Pound")
    @JvmField val LESS_THAN = mirrored("LessThan")
    @JvmField val GREATER_THAN = mirrored("GreaterThan")
    @JvmField val MIXIN_OPERATOR = mirrored("MixinOperator")
    @JvmField val NUMBER = mirrored("Number")
    @JvmField val STRING = mirrored("String")
    @JvmField val SYMBOL = mirrored("Symbol")
    /** A `(* ... *)` comment, nested ones included. The Rust lexer skips comments. */
    @JvmField val COMMENT = NewspeakTokenType("Comment", null)

    /** `newspeak-spec.pdf` §4.1, as `crates/ns-syntax/src/parser/mod.rs`'s `RESERVED_WORDS`. */
    val RESERVED_WORDS: Set<String> = setOf("self", "super", "outer", "true", "false", "nil")
}
```

- [ ] **Step 2: Write the failing lexer tests**

`NewspeakLexerTest.kt` — the Rust lexer's own tests (`crates/ns-syntax/src/lexer.rs`, `mod tests`) ported, plus the cases only this lexer has:

```kotlin
package io.github.bstrauss.newspeak.lexer

import com.intellij.psi.TokenType
import com.intellij.psi.tree.IElementType
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.BAR
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.BINARY_SELECTOR
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.CARET
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.CLASS
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.COLON_COLON_EQUALS
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.COMMENT
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.DOT
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.DOUBLE_BAR
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.EQUALS
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.GREATER_THAN
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.IDENTIFIER
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.KEYWORD
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.LEFT_PARENTHESIS
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.LESS_THAN
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.MIXIN_OPERATOR
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.NUMBER
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.POUND
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.PRIVATE
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.PUBLIC
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.RESERVED_WORD
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.RIGHT_PARENTHESIS
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.SETTER_KEYWORD
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.STRING
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.SYMBOL
import org.junit.Assert.assertEquals
import org.junit.Test

/** Every token of [text] as (type, text), whitespace included. */
fun lexAll(text: String): List<Pair<IElementType, String>> {
    val lexer = NewspeakLexer()
    lexer.start(text, 0, text.length, 0)
    val tokens = mutableListOf<Pair<IElementType, String>>()
    while (true) {
        val type = lexer.tokenType ?: break
        tokens.add(type to text.substring(lexer.tokenStart, lexer.tokenEnd))
        lexer.advance()
    }
    return tokens
}

/** The significant token types of [text]: no whitespace. */
fun kinds(text: String): List<IElementType> =
    lexAll(text).map { (type, _) -> type }.filter { type -> type != TokenType.WHITE_SPACE }

class NewspeakLexerTest {
    @Test fun identifiersAndKeywords() =
        assertEquals(listOf(IDENTIFIER, KEYWORD, IDENTIFIER), kinds("foo bar: baz"))

    @Test fun leadingUnderscoreIdentifier() = assertEquals(listOf(IDENTIFIER), kinds("_x"))

    @Test fun reservedWordsOfTheLexer() =
        assertEquals(listOf(CLASS, PUBLIC, PRIVATE), kinds("class public private"))

    @Test fun reservedWordsOfTheSpecification() =
        assertEquals(List(6) { RESERVED_WORD }, kinds("self super outer true false nil"))

    @Test fun punctuationVersusBinarySelector() =
        assertEquals(listOf(BAR, DOUBLE_BAR, EQUALS, COLON_COLON_EQUALS, CARET, DOT), kinds("| || = ::= ^ ."))

    @Test fun smalltalkAssignmentIsABadCharacterPair() {
        assertEquals(listOf(IDENTIFIER, TokenType.BAD_CHARACTER, NUMBER), kinds("x := 1"))
        assertEquals(TokenType.BAD_CHARACTER to ":=", lexAll("x:=1")[1])
    }

    @Test fun binarySelectors() =
        assertEquals(listOf(IDENTIFIER, BINARY_SELECTOR, IDENTIFIER, BINARY_SELECTOR, IDENTIFIER), kinds("a + b <= c"))

    @Test fun mixinOperatorIsOneTokenAndLeavesLessThanAlone() {
        assertEquals(listOf(IDENTIFIER, MIXIN_OPERATOR, IDENTIFIER), kinds("x <: y"))
        assertEquals(listOf(IDENTIFIER, MIXIN_OPERATOR, IDENTIFIER), kinds("Object<:Value"))
        assertEquals(listOf(IDENTIFIER, LESS_THAN, IDENTIFIER), kinds("x < y"))
        assertEquals(listOf(IDENTIFIER, BINARY_SELECTOR, IDENTIFIER), kinds("x <= y"))
        assertEquals(listOf(IDENTIFIER, LESS_THAN, IDENTIFIER, GREATER_THAN), kinds("foo <Bar>"))
    }

    @Test fun setterKeyword() = assertEquals(listOf(SETTER_KEYWORD, NUMBER), kinds("x:: 3"))

    @Test fun backslashIsABinarySelector() =
        assertEquals(listOf(IDENTIFIER, BINARY_SELECTOR, IDENTIFIER), kinds("a \\\\ b"))

    @Test fun setterKeywordInitializerWithoutSpaces() =
        assertEquals(listOf(IDENTIFIER, COLON_COLON_EQUALS, IDENTIFIER), kinds("foo::=bar"))

    @Test fun decimalNumbers() = assertEquals(List(4) { NUMBER }, kinds("0 42 3.14 1.5e-3"))

    @Test fun radixNumbers() = assertEquals(listOf(NUMBER, NUMBER), kinds("16rFF 2r1010"))

    @Test fun negativeNumberVersusSubtraction() {
        assertEquals(listOf(NUMBER), kinds("-42"))
        assertEquals(listOf(IDENTIFIER, BINARY_SELECTOR, NUMBER), kinds("a - 42"))
        assertEquals(listOf(IDENTIFIER, NUMBER), kinds("a -42"))
        assertEquals(listOf(IDENTIFIER, BINARY_SELECTOR, NUMBER), kinds("a-42"))
    }

    @Test fun aCommentIsNotWhitespaceForTheNegativeLiteralRule() {
        assertEquals(listOf(IDENTIFIER, COMMENT, BINARY_SELECTOR, NUMBER), kinds("x (*c*)-42"))
        assertEquals(listOf(IDENTIFIER, COMMENT, NUMBER), kinds("x (*c*) -42"))
    }

    @Test fun stringsWithEscapedQuote() = assertEquals(listOf(STRING to "'it''s'"), lexAll("'it''s'"))

    @Test fun symbols() = assertEquals(List(4) { SYMBOL }, kinds("#foo #at:put: #+ #'hi there'"))

    @Test fun hashParenthesisIsPoundThenLeftParenthesis() =
        assertEquals(listOf(POUND, LEFT_PARENTHESIS, NUMBER, RIGHT_PARENTHESIS), kinds("#(1)"))

    @Test fun aLoneHashIsASymbol() = assertEquals(listOf(SYMBOL to "#"), lexAll("#"))

    @Test fun doubleQuotedStrings() = assertEquals(List(5) { STRING }, kinds("\"a\" \"'\" \" \" \"abc\" \"\""))

    @Test fun aOneCharacterDoubleQuotedStringMayBeADoubleQuote() =
        assertEquals(STRING to "\"\"\"", lexAll("(char: \"\"\") x").first { (type, _) -> type == STRING })

    @Test fun aOneCharacterDoubleQuotedStringMayBeASurrogatePair() =
        assertEquals(listOf(STRING to "\"😀\""), lexAll("\"😀\""))

    @Test fun unterminatedStringsRunToTheEnd() {
        assertEquals(listOf(STRING to "'oops"), lexAll("'oops"))
        assertEquals(listOf(STRING to "\"oops"), lexAll("\"oops"))
    }

    @Test fun commentsAreTokensAndNest() {
        assertEquals(listOf(IDENTIFIER, COMMENT, IDENTIFIER), kinds("a (* comment *) b"))
        assertEquals(COMMENT to "(* x (* y *) z *)", lexAll("a (* x (* y *) z *) b")[2])
    }

    @Test fun anUnterminatedCommentRunsToTheEnd() =
        assertEquals(COMMENT to "(* oops", lexAll("a (* oops")[2])

    @Test fun aStrayNonAsciiCharacterIsOneBadCharacter() {
        assertEquals(listOf(TokenType.BAD_CHARACTER to "é"), lexAll("é"))
        assertEquals(listOf(TokenType.BAD_CHARACTER to "😀"), lexAll("😀"))
    }

    @Test fun nonAsciiWhitespaceIsABadCharacter() =
        assertEquals(listOf(IDENTIFIER, TokenType.BAD_CHARACTER, IDENTIFIER), kinds("a\u00A0b"))

    @Test fun everyCharacterBelongsToAToken() {
        val text = "class M = Object new ( | x ::= 'é'. | ) ( public f = ( ^x := 😀 (* open"
        assertEquals(text, lexAll(text).joinToString("") { (_, tokenText) -> tokenText })
    }
}
```

`LexerRestartTest.kt`:

```kotlin
package io.github.bstrauss.newspeak.lexer

import org.junit.Assert.assertEquals
import org.junit.Test
import java.nio.file.Files
import java.nio.file.Path

private data class LexedToken(val type: String, val start: Int, val end: Int, val state: Int)

private fun lexFrom(text: String, start: Int, state: Int): List<LexedToken> {
    val lexer = NewspeakLexer()
    lexer.start(text, start, text.length, state)
    val tokens = mutableListOf<LexedToken>()
    while (true) {
        val type = lexer.tokenType ?: break
        tokens.add(LexedToken(type.toString(), lexer.tokenStart, lexer.tokenEnd, lexer.state))
        lexer.advance()
    }
    return tokens
}

/**
 * The editor re-lexes from any token boundary with the state it stored there;
 * doing so must answer exactly the tokens a full lex answers from that point.
 */
fun assertRestartsCorrectly(text: String, context: String) {
    val full = lexFrom(text, 0, 0)
    for ((index, token) in full.withIndex()) {
        assertEquals("$context, restarted at ${token.start}", full.subList(index, full.size), lexFrom(text, token.start, token.state))
    }
}

class LexerRestartTest {
    @Test fun restartsAroundTheNegativeLiteralRule() {
        assertRestartsCorrectly("a -1. a-1. (a) -1. 3 - -1. x (*c*)-42", "fixture")
    }

    @Test fun restartsInsideEveryGolden() {
        val goldens = Path.of(System.getProperty("nsNext.repositoryRoot")).resolve("goldens")
        Files.walk(goldens).use { paths ->
            paths.filter { path -> path.toString().endsWith(".ns") }.forEach { path ->
                assertRestartsCorrectly(Files.readString(path), path.toString())
            }
        }
    }
}
```

- [ ] **Step 3: Pass the repository root to the tests**

In `build.gradle.kts`, append:

```kotlin
val repositoryRoot = layout.projectDirectory.dir("../..").asFile

tasks.test {
    systemProperty("nsNext.repositoryRoot", repositoryRoot.absolutePath)
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains check`
Expected: compilation fails — `Unresolved reference: NewspeakLexer`.

- [ ] **Step 5: Implement the lexer**

`NewspeakLexer.kt`:

```kotlin
package io.github.bstrauss.newspeak.lexer

import com.intellij.lexer.LexerBase
import com.intellij.psi.TokenType
import com.intellij.psi.tree.IElementType
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes as Types

/**
 * The Newspeak lexer, mirroring `crates/ns-syntax/src/lexer.rs` function by
 * function; read the two side by side. The differences, and why, are in
 * `docs/design/editor-support.md` and the plan that built this. The lexer
 * comparison test holds the two to the same tokens over every module in the
 * repository.
 */
class NewspeakLexer : LexerBase() {
    private var buffer: CharSequence = ""
    private var bufferEnd = 0
    private var tokenStart = 0
    private var tokenEnd = 0
    private var currentType: IElementType? = null
    private var stateAtTokenStart = INITIAL
    /** Whether the previous significant token ends a value: the Rust lexer's `previous_ends_value`. */
    private var previousEndsValue = false
    private var position = 0

    override fun start(buffer: CharSequence, startOffset: Int, endOffset: Int, initialState: Int) {
        this.buffer = buffer
        bufferEnd = endOffset
        tokenEnd = startOffset
        previousEndsValue = initialState == PREVIOUS_ENDS_VALUE
        advance()
    }

    override fun getState(): Int = stateAtTokenStart
    override fun getTokenType(): IElementType? = currentType
    override fun getTokenStart(): Int = tokenStart
    override fun getTokenEnd(): Int = tokenEnd
    override fun getBufferSequence(): CharSequence = buffer
    override fun getBufferEnd(): Int = bufferEnd

    override fun advance() {
        tokenStart = tokenEnd
        stateAtTokenStart = if (previousEndsValue) PREVIOUS_ENDS_VALUE else INITIAL
        if (tokenStart >= bufferEnd) {
            currentType = null
            return
        }
        position = tokenStart
        val type = lexOne()
        currentType = type
        tokenEnd = position
        if (type != TokenType.WHITE_SPACE && type != Types.COMMENT && type != TokenType.BAD_CHARACTER) {
            previousEndsValue = type in ENDS_VALUE
        }
    }

    private fun peek(offset: Int = 0): Char? {
        val index = position + offset
        return if (index < bufferEnd) buffer[index] else null
    }

    /** One code point's length in chars at [index], never reaching past the end. */
    private fun codePointLength(index: Int): Int =
        minOf(Character.charCount(Character.codePointAt(buffer, index)), bufferEnd - index)

    private fun single(type: IElementType): IElementType {
        position += 1
        return type
    }

    private fun lexOne(): IElementType {
        val character = buffer[position]
        // `skip_trivia`: whitespace and comments, which the Rust lexer drops.
        if (isAsciiWhitespace(character)) {
            while (peek()?.let { next -> isAsciiWhitespace(next) } == true) position += 1
            return TokenType.WHITE_SPACE
        }
        if (character == '(' && peek(1) == '*') {
            lexBlockComment()
            return Types.COMMENT
        }
        // `lex_one`, arm by arm.
        return when {
            character == '(' -> single(Types.LEFT_PARENTHESIS)
            character == ')' -> single(Types.RIGHT_PARENTHESIS)
            character == '[' -> single(Types.LEFT_BRACKET)
            character == ']' -> single(Types.RIGHT_BRACKET)
            character == '{' -> single(Types.LEFT_BRACE)
            character == '}' -> single(Types.RIGHT_BRACE)
            character == '^' -> single(Types.CARET)
            character == '.' -> single(Types.DOT)
            character == ';' -> single(Types.SEMICOLON)
            character == '#' && peek(1) == '(' -> single(Types.POUND)
            character == '#' -> lexSymbol()
            character == '|' -> when {
                peek(1) == '|' -> {
                    position += 2
                    Types.DOUBLE_BAR
                }
                peek(1)?.let { next -> next in SPECIAL } == true -> lexOperator()
                else -> single(Types.BAR)
            }
            character == ':' -> when {
                peek(1) == ':' && peek(2) == '=' -> {
                    position += 3
                    Types.COLON_COLON_EQUALS
                }
                peek(1) == '=' -> {
                    // `:=`: the Rust lexer reports it and emits no token.
                    position += 2
                    TokenType.BAD_CHARACTER
                }
                else -> single(Types.COLON)
            }
            character == '_' || isAsciiLetter(character) -> lexWord()
            isAsciiDigit(character) -> lexNumber()
            character == '\'' -> lexString()
            character == '"' -> lexDoubleQuotedString()
            character == '-' &&
                peek(1)?.let { next -> isAsciiDigit(next) } == true &&
                (!previousEndsValue || precededByWhitespace()) -> lexNumber()
            character in SPECIAL -> lexOperator()
            else -> {
                // A character that starts no token: the Rust lexer reports it
                // and steps over one code point, emitting no token.
                position += codePointLength(position)
                TokenType.BAD_CHARACTER
            }
        }
    }

    private fun lexBlockComment() {
        position += 2
        var depth = 1
        while (depth > 0) {
            val current = peek() ?: return
            when {
                current == '(' && peek(1) == '*' -> {
                    position += 2
                    depth += 1
                }
                current == '*' && peek(1) == ')' -> {
                    position += 2
                    depth -= 1
                }
                else -> position += 1
            }
        }
    }

    private fun lexWord(): IElementType {
        val start = position
        while (peek()?.let { next -> isWordCharacter(next) } == true) position += 1
        if (peek() == ':') {
            if (peek(1) == '=' || (peek(1) == ':' && peek(2) == '=')) {
                return wordKind(start, position)
            }
            if (peek(1) == ':') {
                position += 2
                return Types.SETTER_KEYWORD
            }
            position += 1
            return Types.KEYWORD
        }
        return wordKind(start, position)
    }

    private fun wordKind(start: Int, end: Int): IElementType =
        when (val word = buffer.subSequence(start, end).toString()) {
            "class" -> Types.CLASS
            "private" -> Types.PRIVATE
            "public" -> Types.PUBLIC
            "protected" -> Types.PROTECTED
            in Types.RESERVED_WORDS -> Types.RESERVED_WORD
            else -> if (word.isEmpty()) TokenType.BAD_CHARACTER else Types.IDENTIFIER
        }

    private fun lexOperator(): IElementType {
        val character = buffer[position]
        val next = peek(1)
        if (character == '<' && next == ':') {
            position += 2
            return Types.MIXIN_OPERATOR
        }
        val runOfTwoPlus = next != null && next in SPECIAL
        if (!runOfTwoPlus) {
            when (character) {
                '=' -> return single(Types.EQUALS)
                '<' -> return single(Types.LESS_THAN)
                '>' -> return single(Types.GREATER_THAN)
            }
        }
        while (peek()?.let { following -> following in SPECIAL } == true) position += 1
        return Types.BINARY_SELECTOR
    }

    private fun lexNumber(): IElementType {
        if (peek() == '-') position += 1
        eatDigits()
        if (peek() == 'r') {
            position += 1
            if (peek() == '-') position += 1
            eatExtendedDigits()
            if (peek() == '.' && peek(1)?.let { next -> isAsciiLetterOrDigit(next) } == true) {
                position += 1
                eatExtendedDigits()
            }
        } else if (peek() == '.' && peek(1)?.let { next -> isAsciiDigit(next) } == true) {
            position += 1
            eatDigits()
        }
        if (peek() == 'e' && exponentFollows()) {
            position += 1
            if (peek() == '-') position += 1
            eatDigits()
        }
        return Types.NUMBER
    }

    private fun exponentFollows(): Boolean {
        var offset = 1
        if (peek(offset) == '-') offset += 1
        return peek(offset)?.let { next -> isAsciiDigit(next) } == true
    }

    private fun eatDigits() {
        while (peek()?.let { next -> isAsciiDigit(next) } == true) position += 1
    }

    private fun eatExtendedDigits() {
        while (peek()?.let { next -> isAsciiDigit(next) || next in 'A'..'Z' } == true) position += 1
    }

    /** A single-quoted string, `''` inside it a quote; unterminated, it runs to the end. */
    private fun lexString(): IElementType {
        position += 1
        while (true) {
            when (peek()) {
                null -> break
                '\'' -> if (peek(1) == '\'') {
                    position += 2
                } else {
                    position += 1
                    break
                }
                else -> position += 1
            }
        }
        return Types.STRING
    }

    private fun lexSymbol(): IElementType {
        position += 1
        val next = peek()
        when {
            next == '\'' -> lexString()
            next != null && (next == '_' || isAsciiLetter(next)) -> {
                while (peek()?.let { following -> isWordCharacter(following) } == true) position += 1
                while (peek() == ':') {
                    position += 1
                    while (peek()?.let { following -> isWordCharacter(following) } == true) position += 1
                }
            }
            next != null && next in SPECIAL -> {
                while (peek()?.let { following -> following in SPECIAL } == true) position += 1
            }
            // A `#` followed by nothing a symbol can be: the Rust lexer reports
            // it and still emits the `#` as a symbol.
        }
        return Types.SYMBOL
    }

    /**
     * A double-quoted string (§5.1.5), which has no escape: any one character
     * between two quotes, `"` included, else everything up to the next `"`.
     */
    private fun lexDoubleQuotedString(): IElementType {
        position += 1
        if (position < bufferEnd) {
            val after = position + codePointLength(position)
            if (after < bufferEnd && buffer[after] == '"') {
                position = after + 1
                return Types.STRING
            }
        }
        while (position < bufferEnd && buffer[position] != '"') position += 1
        if (position < bufferEnd) position += 1
        return Types.STRING
    }

    /** The Rust lexer's `preceded_by_whitespace`: a comment does not count. */
    private fun precededByWhitespace(): Boolean = position == 0 || isAsciiWhitespace(buffer[position - 1])

    companion object {
        const val INITIAL = 0
        const val PREVIOUS_ENDS_VALUE = 1

        /** The Rust lexer's `SPECIAL`. */
        private const val SPECIAL = "+-*/\\~<>=&|@%,?!"

        private val ENDS_VALUE: Set<IElementType> = setOf(
            Types.IDENTIFIER, Types.RESERVED_WORD, Types.NUMBER, Types.STRING, Types.SYMBOL,
            Types.RIGHT_PARENTHESIS, Types.RIGHT_BRACKET, Types.RIGHT_BRACE,
        )

        /** Rust's `u8::is_ascii_whitespace`: no vertical tab, and nothing beyond ASCII. */
        private fun isAsciiWhitespace(character: Char) =
            character == ' ' || character == '\t' || character == '\n' || character == '\u000C' || character == '\r'

        private fun isAsciiLetter(character: Char) = character in 'a'..'z' || character in 'A'..'Z'
        private fun isAsciiDigit(character: Char) = character in '0'..'9'
        private fun isAsciiLetterOrDigit(character: Char) = isAsciiLetter(character) || isAsciiDigit(character)
        private fun isWordCharacter(character: Char) = character == '_' || isAsciiLetterOrDigit(character)
    }
}
```

`wordKind`'s empty-word branch never fires (`lexWord` is only entered on a word character) but keeps the function total.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains check`
Expected: `BUILD SUCCESSFUL`; every test in `NewspeakLexerTest` and `LexerRestartTest` passes. A failing Rust-ported case means this lexer departs from `lexer.rs`: fix the lexer, not the case.

- [ ] **Step 7: Commit**

```bash
git add editors/jetbrains
git commit -m "feat: Kotlin lexer mirroring ns-syntax's"
```

---

### Task 3: The lexer comparison gate

**Files:**
- Modify: `editors/jetbrains/build.gradle.kts`
- Create: `src/test/kotlin/io/github/bstrauss/newspeak/lexer/LexerComparisonTest.kt`

**Interfaces:**
- Consumes: `ns-next-lsp dump-tokens` (Part 1, Task 5): one JSON line per file, `{"file", "tokens": [{"kind", "start", "end"}], "diagnostics"}`, UTF-16 offsets, `EndOfInput` omitted.

- [ ] **Step 1: Build the debug server for the tests**

Append to `build.gradle.kts`:

```kotlin
val executableSuffix = if (System.getProperty("os.name").startsWith("Windows")) ".exe" else ""

/** The debug build is what the tests use: the release profile's fat LTO makes that build slow. */
val buildLanguageServerDebug by tasks.registering(Exec::class) {
    description = "Builds ns-next-lsp (debug) with Cargo for the tests."
    workingDir = repositoryRoot
    commandLine("cargo", "build", "-p", "ns-lsp")
}

tasks.test {
    dependsOn(buildLanguageServerDebug)
    systemProperty(
        "nsNext.languageServer",
        repositoryRoot.resolve("target/debug/ns-next-lsp$executableSuffix").absolutePath,
    )
}
```

(`tasks.test { }` may be merged with the block from Task 2.)

- [ ] **Step 2: Write the comparison test**

`LexerComparisonTest.kt`:

```kotlin
package io.github.bstrauss.newspeak.lexer

import com.google.gson.JsonParser
import com.intellij.psi.TokenType
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.TimeUnit
import kotlin.io.path.extension
import kotlin.io.path.isRegularFile

private data class ComparedToken(val kind: String, val start: Int, val end: Int)

/**
 * The Kotlin lexer against `ns-syntax`'s, over every module the repository
 * holds: every significant token must agree in kind and span, and the tokens
 * only the Kotlin lexer has -- whitespace and comments -- must fill exactly
 * the gaps between them. This is what keeps the second lexer honest
 * (`docs/design/editor-support.md`, "Why a Kotlin lexer").
 */
class LexerComparisonTest {
    private val repositoryRoot: Path = Path.of(System.getProperty("nsNext.repositoryRoot"))
    private val languageServer: Path = Path.of(System.getProperty("nsNext.languageServer"))

    private fun modules(): List<Path> {
        val files = mutableListOf<Path>()
        for (directory in listOf("corpus/modules", "goldens")) {
            Files.walk(repositoryRoot.resolve(directory)).use { paths ->
                paths.filter { path -> path.isRegularFile() && path.extension == "ns" }.forEach { path -> files.add(path) }
            }
        }
        files.add(repositoryRoot.resolve("crates/ns-runtime/core-library/Kernel.ns"))
        return files.sorted()
    }

    private fun rustTokens(files: List<Path>): Map<String, Pair<List<ComparedToken>, Int>> {
        val output = Files.createTempFile("dump-tokens", ".jsonl")
        try {
            val process = ProcessBuilder(listOf(languageServer.toString(), "dump-tokens") + files.map { file -> file.toString() })
                .redirectOutput(output.toFile())
                .redirectError(ProcessBuilder.Redirect.INHERIT)
                .start()
            assertTrue("dump-tokens finishes", process.waitFor(5, TimeUnit.MINUTES))
            assertEquals("dump-tokens exit status", 0, process.exitValue())
            return Files.readAllLines(output).associate { line ->
                val entry = JsonParser.parseString(line).asJsonObject
                val tokens = entry.getAsJsonArray("tokens").map { element ->
                    val token = element.asJsonObject
                    ComparedToken(token.get("kind").asString, token.get("start").asInt, token.get("end").asInt)
                }
                entry.get("file").asString to (tokens to entry.get("diagnostics").asInt)
            }
        } finally {
            Files.deleteIfExists(output)
        }
    }

    @Test
    fun theKotlinLexerAgreesWithNsSyntax() {
        val files = modules()
        assertTrue("found only ${files.size} modules", files.size > 200)
        val expected = rustTokens(files)
        for (file in files) {
            // Read exactly as the Rust lexer reads it: no line-ending conversion.
            val text = Files.readString(file)
            val (rustTokens, diagnostics) = expected.getValue(file.toString())
            val lexer = NewspeakLexer()
            lexer.start(text, 0, text.length, 0)
            val significant = mutableListOf<ComparedToken>()
            var badCharacters = 0
            var covered = 0
            while (true) {
                val type = lexer.tokenType ?: break
                assertEquals("$file: tokens are contiguous", covered, lexer.tokenStart)
                covered = lexer.tokenEnd
                val tokenText = text.substring(lexer.tokenStart, lexer.tokenEnd)
                when {
                    type == TokenType.WHITE_SPACE ->
                        assertTrue("$file: whitespace at ${lexer.tokenStart}", tokenText.all { character -> character in " \t\n\u000C\r" })
                    type == NewspeakTokenTypes.COMMENT ->
                        assertTrue("$file: comment at ${lexer.tokenStart}", tokenText.startsWith("(*"))
                    type == TokenType.BAD_CHARACTER -> badCharacters += 1
                    else -> significant.add(ComparedToken((type as NewspeakTokenType).rustKind!!, lexer.tokenStart, lexer.tokenEnd))
                }
                lexer.advance()
            }
            assertEquals("$file: the whole text is covered", text.length, covered)
            assertEquals("$file", rustTokens, significant)
            if (diagnostics == 0) {
                assertEquals("$file: no bad character where ns-syntax reports nothing", 0, badCharacters)
            }
        }
    }
}
```

Gson is on the platform's classpath, so it needs no dependency. If the IDE's bundled Gson is not visible to tests, add `testImplementation("com.google.code.gson:gson:2.13.1")`.

`!!` on `rustKind` is safe here: only `COMMENT` has a null `rustKind`, and it is handled above.

- [ ] **Step 3: Run it**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains check`
Expected: `cargo build -p ns-lsp` runs first, then `BUILD SUCCESSFUL`, `LexerComparisonTest` passing. A failure names the file and the first differing token: fix the Kotlin lexer. If `ns-syntax`'s lexer is the one that looks wrong, stop and report it to the controller — it is pinned by the runtime's goldens and not changed from here.

- [ ] **Step 4: Commit**

```bash
git add editors/jetbrains
git commit -m "test: compare the Kotlin lexer with ns-syntax's over every module"
```

---

### Task 4: Highlighting and the editor features

**Files:**
- Create: `highlighting/NewspeakColors.kt`, `highlighting/NewspeakSyntaxHighlighter.kt`, `highlighting/NewspeakColorSettingsPage.kt`
- Create: `parser/NewspeakParserDefinition.kt`
- Create: `editor/NewspeakBraceMatcher.kt`, `editor/NewspeakCommenter.kt`, `editor/NewspeakQuoteHandler.kt`, `editor/NewspeakIndexPatternBuilder.kt`, `editor/NewspeakSpellcheckingStrategy.kt`
- Modify: `src/main/resources/META-INF/plugin.xml`
- Modify: `src/test/kotlin/io/github/bstrauss/newspeak/NewspeakEditorTest.kt`
- Create: `src/test/kotlin/io/github/bstrauss/newspeak/highlighting/NewspeakColorSettingsPageTest.kt`

**Interfaces:**
- Consumes: `NewspeakLexer`, `NewspeakTokenTypes` (Task 2).
- Produces: `object NewspeakColors` with the `TextAttributesKey`s below — Task 5 maps semantic tokens onto `PARAMETER`, `LOCAL_VARIABLE`, `SLOT`, `IMMUTABLE_SLOT`, `CLASS_NAME`, `METHOD_DECLARATION` and `SEND`.

- [ ] **Step 1: Write the failing tests**

Extend `NewspeakEditorTest`:

```kotlin
package io.github.bstrauss.newspeak

import com.intellij.codeInsight.highlighting.BraceMatchingUtil
import com.intellij.openapi.actionSystem.IdeActions
import com.intellij.openapi.fileTypes.FileTypeManager
import com.intellij.psi.search.PsiTodoSearchHelper
import com.intellij.testFramework.fixtures.BasePlatformTestCase

class NewspeakEditorTest : BasePlatformTestCase() {
    fun testNsFilesAreNewspeak() {
        assertSame(NewspeakFileType, FileTypeManager.getInstance().getFileTypeByFileName("Module.ns"))
    }

    fun testTheFileParsesIntoANewspeakFile() {
        val file = myFixture.configureByText("Module.ns", "class M = Object new () ()")
        assertInstanceOf(file, NewspeakFile::class.java)
    }

    fun testParenthesesMatch() {
        myFixture.configureByText("Module.ns", "f = <caret>( ^[ 1 ] )")
        val editor = myFixture.editor
        val offset = editor.caretModel.offset
        val iterator = (editor as com.intellij.openapi.editor.ex.EditorEx).highlighter.createIterator(offset)
        assertTrue(BraceMatchingUtil.matchBrace(editor.document.charsSequence, myFixture.file.fileType, iterator, true))
        assertEquals(editor.document.text.lastIndexOf(')'), iterator.start)
    }

    fun testBlockCommentWrapsTheSelection() {
        myFixture.configureByText("Module.ns", "<selection>x</selection>")
        myFixture.performEditorAction(IdeActions.ACTION_COMMENT_BLOCK)
        myFixture.checkResult("(*x*)")
    }

    fun testTodoInACommentIsFound() {
        myFixture.configureByText("Module.ns", "(* TODO: finish *) x")
        assertEquals(1, PsiTodoSearchHelper.getInstance(project).findTodoItems(myFixture.file).size)
    }
}
```

`NewspeakColorSettingsPageTest.kt`:

```kotlin
package io.github.bstrauss.newspeak.highlighting

import com.intellij.psi.TokenType
import io.github.bstrauss.newspeak.lexer.lexAll
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class NewspeakColorSettingsPageTest {
    private val page = NewspeakColorSettingsPage()

    @Test fun theDemoTextHasNoBadCharacter() {
        val text = page.getDemoText().replace(Regex("</?[a-z]+>"), "")
        assertTrue(lexAll(text).none { (type, _) -> type == TokenType.BAD_CHARACTER })
    }

    @Test fun everyTagInTheDemoTextIsKnown() {
        val tags = Regex("<([a-z]+)>").findAll(page.getDemoText()).map { match -> match.groupValues[1] }.toSet()
        assertEquals(page.getAdditionalHighlightingTagToDescriptorMap().keys, tags)
    }

    @Test fun everyKeyHasADescriptor() {
        val described = page.getAttributeDescriptors().map { descriptor -> descriptor.key }.toSet()
        assertEquals(NewspeakColors.ALL.toSet(), described)
    }
}
```

`lexAll` is the helper from Task 2's `NewspeakLexerTest.kt`, a top-level function in the same test source set.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains check`
Expected: compilation fails — `Unresolved reference: NewspeakColorSettingsPage`.

- [ ] **Step 3: Colours and the syntax highlighter**

`NewspeakColors.kt`:

```kotlin
package io.github.bstrauss.newspeak.highlighting

import com.intellij.openapi.editor.DefaultLanguageHighlighterColors as Default
import com.intellij.openapi.editor.HighlighterColors
import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.editor.colors.TextAttributesKey.createTextAttributesKey

/**
 * Every colour the plugin sets, each derived from a platform default so every
 * colour scheme works without configuration. The first group comes from the
 * lexer; the second from the language server's semantic tokens.
 */
object NewspeakColors {
    @JvmField val KEYWORD = createTextAttributesKey("NEWSPEAK_KEYWORD", Default.KEYWORD)
    @JvmField val RESERVED_WORD = createTextAttributesKey("NEWSPEAK_RESERVED_WORD", Default.KEYWORD)
    @JvmField val RETURN = createTextAttributesKey("NEWSPEAK_RETURN", Default.KEYWORD)
    @JvmField val IDENTIFIER = createTextAttributesKey("NEWSPEAK_IDENTIFIER", Default.IDENTIFIER)
    @JvmField val KEYWORD_SELECTOR = createTextAttributesKey("NEWSPEAK_KEYWORD_SELECTOR", Default.FUNCTION_CALL)
    @JvmField val BINARY_SELECTOR = createTextAttributesKey("NEWSPEAK_BINARY_SELECTOR", Default.OPERATION_SIGN)
    @JvmField val ASSIGNMENT = createTextAttributesKey("NEWSPEAK_ASSIGNMENT", Default.OPERATION_SIGN)
    @JvmField val NUMBER = createTextAttributesKey("NEWSPEAK_NUMBER", Default.NUMBER)
    @JvmField val STRING = createTextAttributesKey("NEWSPEAK_STRING", Default.STRING)
    @JvmField val SYMBOL = createTextAttributesKey("NEWSPEAK_SYMBOL", Default.CONSTANT)
    @JvmField val COMMENT = createTextAttributesKey("NEWSPEAK_COMMENT", Default.BLOCK_COMMENT)
    @JvmField val PARENTHESES = createTextAttributesKey("NEWSPEAK_PARENTHESES", Default.PARENTHESES)
    @JvmField val BRACKETS = createTextAttributesKey("NEWSPEAK_BRACKETS", Default.BRACKETS)
    @JvmField val BRACES = createTextAttributesKey("NEWSPEAK_BRACES", Default.BRACES)
    @JvmField val SLOT_BARS = createTextAttributesKey("NEWSPEAK_SLOT_BARS", Default.BRACKETS)
    @JvmField val DOT = createTextAttributesKey("NEWSPEAK_DOT", Default.DOT)
    @JvmField val SEMICOLON = createTextAttributesKey("NEWSPEAK_SEMICOLON", Default.SEMICOLON)
    @JvmField val BAD_CHARACTER = createTextAttributesKey("NEWSPEAK_BAD_CHARACTER", HighlighterColors.BAD_CHARACTER)

    @JvmField val PARAMETER = createTextAttributesKey("NEWSPEAK_PARAMETER", Default.PARAMETER)
    @JvmField val LOCAL_VARIABLE = createTextAttributesKey("NEWSPEAK_LOCAL_VARIABLE", Default.LOCAL_VARIABLE)
    @JvmField val SLOT = createTextAttributesKey("NEWSPEAK_SLOT", Default.INSTANCE_FIELD)
    @JvmField val IMMUTABLE_SLOT = createTextAttributesKey("NEWSPEAK_IMMUTABLE_SLOT", Default.INSTANCE_FIELD)
    @JvmField val CLASS_NAME = createTextAttributesKey("NEWSPEAK_CLASS_NAME", Default.CLASS_NAME)
    @JvmField val METHOD_DECLARATION = createTextAttributesKey("NEWSPEAK_METHOD_DECLARATION", Default.FUNCTION_DECLARATION)
    @JvmField val SEND = createTextAttributesKey("NEWSPEAK_SEND", Default.FUNCTION_CALL)

    val ALL: List<TextAttributesKey> = listOf(
        KEYWORD, RESERVED_WORD, RETURN, IDENTIFIER, KEYWORD_SELECTOR, BINARY_SELECTOR, ASSIGNMENT,
        NUMBER, STRING, SYMBOL, COMMENT, PARENTHESES, BRACKETS, BRACES, SLOT_BARS, DOT, SEMICOLON,
        BAD_CHARACTER, PARAMETER, LOCAL_VARIABLE, SLOT, IMMUTABLE_SLOT, CLASS_NAME, METHOD_DECLARATION, SEND,
    )
}
```

`NewspeakSyntaxHighlighter.kt`:

```kotlin
package io.github.bstrauss.newspeak.highlighting

import com.intellij.lexer.Lexer
import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.fileTypes.SyntaxHighlighter
import com.intellij.openapi.fileTypes.SyntaxHighlighterBase
import com.intellij.openapi.fileTypes.SyntaxHighlighterFactory
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.psi.TokenType
import com.intellij.psi.tree.IElementType
import io.github.bstrauss.newspeak.lexer.NewspeakLexer
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes as Types

class NewspeakSyntaxHighlighter : SyntaxHighlighterBase() {
    override fun getHighlightingLexer(): Lexer = NewspeakLexer()

    override fun getTokenHighlights(tokenType: IElementType?): Array<TextAttributesKey> =
        pack(ATTRIBUTES[tokenType])

    private companion object {
        val ATTRIBUTES: Map<IElementType, TextAttributesKey> = mapOf(
            Types.CLASS to NewspeakColors.KEYWORD,
            Types.PUBLIC to NewspeakColors.KEYWORD,
            Types.PRIVATE to NewspeakColors.KEYWORD,
            Types.PROTECTED to NewspeakColors.KEYWORD,
            Types.RESERVED_WORD to NewspeakColors.RESERVED_WORD,
            Types.CARET to NewspeakColors.RETURN,
            Types.IDENTIFIER to NewspeakColors.IDENTIFIER,
            Types.KEYWORD to NewspeakColors.KEYWORD_SELECTOR,
            Types.SETTER_KEYWORD to NewspeakColors.KEYWORD_SELECTOR,
            Types.BINARY_SELECTOR to NewspeakColors.BINARY_SELECTOR,
            Types.LESS_THAN to NewspeakColors.BINARY_SELECTOR,
            Types.GREATER_THAN to NewspeakColors.BINARY_SELECTOR,
            Types.EQUALS to NewspeakColors.ASSIGNMENT,
            Types.COLON_COLON_EQUALS to NewspeakColors.ASSIGNMENT,
            Types.MIXIN_OPERATOR to NewspeakColors.ASSIGNMENT,
            Types.NUMBER to NewspeakColors.NUMBER,
            Types.STRING to NewspeakColors.STRING,
            Types.SYMBOL to NewspeakColors.SYMBOL,
            Types.POUND to NewspeakColors.SYMBOL,
            Types.COMMENT to NewspeakColors.COMMENT,
            Types.LEFT_PARENTHESIS to NewspeakColors.PARENTHESES,
            Types.RIGHT_PARENTHESIS to NewspeakColors.PARENTHESES,
            Types.LEFT_BRACKET to NewspeakColors.BRACKETS,
            Types.RIGHT_BRACKET to NewspeakColors.BRACKETS,
            Types.LEFT_BRACE to NewspeakColors.BRACES,
            Types.RIGHT_BRACE to NewspeakColors.BRACES,
            Types.BAR to NewspeakColors.SLOT_BARS,
            Types.DOUBLE_BAR to NewspeakColors.SLOT_BARS,
            Types.DOT to NewspeakColors.DOT,
            Types.SEMICOLON to NewspeakColors.SEMICOLON,
            TokenType.BAD_CHARACTER to NewspeakColors.BAD_CHARACTER,
        )
    }
}

class NewspeakSyntaxHighlighterFactory : SyntaxHighlighterFactory() {
    override fun getSyntaxHighlighter(project: Project?, virtualFile: VirtualFile?): SyntaxHighlighter =
        NewspeakSyntaxHighlighter()
}
```

- [ ] **Step 4: The colour settings page**

`NewspeakColorSettingsPage.kt`:

```kotlin
package io.github.bstrauss.newspeak.highlighting

import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.fileTypes.SyntaxHighlighter
import com.intellij.openapi.options.colors.AttributesDescriptor
import com.intellij.openapi.options.colors.ColorDescriptor
import com.intellij.openapi.options.colors.ColorSettingsPage
import io.github.bstrauss.newspeak.NewspeakIcons
import javax.swing.Icon

class NewspeakColorSettingsPage : ColorSettingsPage {
    override fun getDisplayName(): String = "Newspeak"
    override fun getIcon(): Icon = NewspeakIcons.FILE
    override fun getHighlighter(): SyntaxHighlighter = NewspeakSyntaxHighlighter()
    override fun getColorDescriptors(): Array<ColorDescriptor> = ColorDescriptor.EMPTY_ARRAY

    override fun getAttributeDescriptors(): Array<AttributesDescriptor> = arrayOf(
        AttributesDescriptor("Keywords//class and access modifiers", NewspeakColors.KEYWORD),
        AttributesDescriptor("Keywords//Reserved words (self, super, outer, true, false, nil)", NewspeakColors.RESERVED_WORD),
        AttributesDescriptor("Keywords//Return (^)", NewspeakColors.RETURN),
        AttributesDescriptor("Identifier", NewspeakColors.IDENTIFIER),
        AttributesDescriptor("Selectors//Keyword selector", NewspeakColors.KEYWORD_SELECTOR),
        AttributesDescriptor("Selectors//Binary selector", NewspeakColors.BINARY_SELECTOR),
        AttributesDescriptor("Operators//Assignment and mixin application (=, ::=, <:)", NewspeakColors.ASSIGNMENT),
        AttributesDescriptor("Literals//Number", NewspeakColors.NUMBER),
        AttributesDescriptor("Literals//String", NewspeakColors.STRING),
        AttributesDescriptor("Literals//Symbol", NewspeakColors.SYMBOL),
        AttributesDescriptor("Comment", NewspeakColors.COMMENT),
        AttributesDescriptor("Braces//Parentheses", NewspeakColors.PARENTHESES),
        AttributesDescriptor("Braces//Brackets", NewspeakColors.BRACKETS),
        AttributesDescriptor("Braces//Braces", NewspeakColors.BRACES),
        AttributesDescriptor("Braces//Slot bars", NewspeakColors.SLOT_BARS),
        AttributesDescriptor("Punctuation//Dot", NewspeakColors.DOT),
        AttributesDescriptor("Punctuation//Semicolon", NewspeakColors.SEMICOLON),
        AttributesDescriptor("Bad character", NewspeakColors.BAD_CHARACTER),
        AttributesDescriptor("Semantic//Parameter", NewspeakColors.PARAMETER),
        AttributesDescriptor("Semantic//Local variable", NewspeakColors.LOCAL_VARIABLE),
        AttributesDescriptor("Semantic//Slot", NewspeakColors.SLOT),
        AttributesDescriptor("Semantic//Immutable slot", NewspeakColors.IMMUTABLE_SLOT),
        AttributesDescriptor("Semantic//Class", NewspeakColors.CLASS_NAME),
        AttributesDescriptor("Semantic//Method declaration", NewspeakColors.METHOD_DECLARATION),
        AttributesDescriptor("Semantic//Send", NewspeakColors.SEND),
    )

    override fun getAdditionalHighlightingTagToDescriptorMap(): Map<String, TextAttributesKey> = mapOf(
        "parameter" to NewspeakColors.PARAMETER,
        "local" to NewspeakColors.LOCAL_VARIABLE,
        "slot" to NewspeakColors.SLOT,
        "immutable" to NewspeakColors.IMMUTABLE_SLOT,
        "class" to NewspeakColors.CLASS_NAME,
        "declaration" to NewspeakColors.METHOD_DECLARATION,
        "send" to NewspeakColors.SEND,
    )

    override fun getDemoText(): String = """
        Newspeak3
        'Examples'
        class <class>Counter</class> usingPlatform: <parameter>platform</parameter> = Object new (
          (* A counter. TODO: reset *)
          | private <slot>count</slot> ::= 0. private <immutable>limit</immutable> = 16rFF. |
        ) (
          public <declaration>increment:</declaration> <parameter>step</parameter> = (
            | <local>next</local> |
            <local>next</local>:: <slot>count</slot> + <parameter>step</parameter>.
            <local>next</local> > <immutable>limit</immutable> ifTrue: [^nil].
            <slot>count</slot>:: <local>next</local>.
            ^self <send>report:</send> #counted with: 'done' and: "!"
          )
          public <declaration>report:</declaration> <parameter>what</parameter> with: <parameter>text</parameter> and: <parameter>mark</parameter> = ( ^{ <parameter>what</parameter>. <parameter>text</parameter>. <parameter>mark</parameter> } )
        ) : ()
    """.trimIndent()
}
```

`everyKeyHasADescriptor` pins that every key in `NewspeakColors.ALL` is configurable here.

- [ ] **Step 5: The flat parser definition**

`NewspeakParserDefinition.kt`:

```kotlin
package io.github.bstrauss.newspeak.parser

import com.intellij.extapi.psi.ASTWrapperPsiElement
import com.intellij.lang.ASTNode
import com.intellij.lang.ParserDefinition
import com.intellij.lang.PsiParser
import com.intellij.lexer.Lexer
import com.intellij.openapi.project.Project
import com.intellij.psi.FileViewProvider
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiFile
import com.intellij.psi.tree.IFileElementType
import com.intellij.psi.tree.TokenSet
import io.github.bstrauss.newspeak.NewspeakFile
import io.github.bstrauss.newspeak.NewspeakLanguage
import io.github.bstrauss.newspeak.lexer.NewspeakLexer
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes

/**
 * Every token under one file node, with no grammar: this is deliberately not
 * a second Newspeak parser (`docs/design/editor-support.md`). It exists so the
 * platform knows which tokens are comments and strings -- for TODO markers,
 * spell checking and the quote handler. Structure comes from the language
 * server.
 */
class NewspeakParserDefinition : ParserDefinition {
    override fun createLexer(project: Project?): Lexer = NewspeakLexer()

    override fun createParser(project: Project?): PsiParser = PsiParser { root, builder ->
        val marker = builder.mark()
        while (!builder.eof()) builder.advanceLexer()
        marker.done(root)
        builder.treeBuilt
    }

    override fun getFileNodeType(): IFileElementType = FILE
    override fun getCommentTokens(): TokenSet = COMMENTS
    override fun getStringLiteralElements(): TokenSet = STRINGS
    override fun createElement(node: ASTNode): PsiElement = ASTWrapperPsiElement(node)
    override fun createFile(viewProvider: FileViewProvider): PsiFile = NewspeakFile(viewProvider)

    companion object {
        @JvmField val FILE = IFileElementType(NewspeakLanguage)
        @JvmField val COMMENTS: TokenSet = TokenSet.create(NewspeakTokenTypes.COMMENT)
        @JvmField val STRINGS: TokenSet = TokenSet.create(NewspeakTokenTypes.STRING)
    }
}
```

- [ ] **Step 6: Braces, comments, quotes, TODO, spell checking**

`NewspeakBraceMatcher.kt`:

```kotlin
package io.github.bstrauss.newspeak.editor

import com.intellij.lang.BracePair
import com.intellij.lang.PairedBraceMatcher
import com.intellij.psi.PsiFile
import com.intellij.psi.tree.IElementType
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes as Types

/**
 * `()`, `[]` and `{}`. Not `<>`: `<` and `>` are also binary selectors, and
 * pairing a comparison with a type's closing bracket would be worse than
 * pairing nothing.
 */
class NewspeakBraceMatcher : PairedBraceMatcher {
    override fun getPairs(): Array<BracePair> = arrayOf(
        BracePair(Types.LEFT_PARENTHESIS, Types.RIGHT_PARENTHESIS, false),
        BracePair(Types.LEFT_BRACKET, Types.RIGHT_BRACKET, false),
        BracePair(Types.LEFT_BRACE, Types.RIGHT_BRACE, false),
    )

    override fun isPairedBracesAllowedBeforeType(lbraceType: IElementType, contextType: IElementType?): Boolean = true

    override fun getCodeConstructStart(file: PsiFile?, openingBraceOffset: Int): Int = openingBraceOffset
}
```

(The parameter names `lbraceType` and `contextType` are the interface's own and must match it.)

`NewspeakCommenter.kt`:

```kotlin
package io.github.bstrauss.newspeak.editor

import com.intellij.lang.Commenter

/** Newspeak has block comments only. */
class NewspeakCommenter : Commenter {
    override fun getLineCommentPrefix(): String? = null
    override fun getBlockCommentPrefix(): String = "(*"
    override fun getBlockCommentSuffix(): String = "*)"
    override fun getCommentedBlockCommentPrefix(): String? = null
    override fun getCommentedBlockCommentSuffix(): String? = null
}
```

`NewspeakQuoteHandler.kt`:

```kotlin
package io.github.bstrauss.newspeak.editor

import com.intellij.codeInsight.editorActions.SimpleTokenSetQuoteHandler
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes

class NewspeakQuoteHandler : SimpleTokenSetQuoteHandler(NewspeakTokenTypes.STRING)
```

`NewspeakIndexPatternBuilder.kt`:

```kotlin
package io.github.bstrauss.newspeak.editor

import com.intellij.lexer.Lexer
import com.intellij.psi.PsiFile
import com.intellij.psi.impl.search.IndexPatternBuilder
import com.intellij.psi.tree.IElementType
import com.intellij.psi.tree.TokenSet
import io.github.bstrauss.newspeak.NewspeakFile
import io.github.bstrauss.newspeak.lexer.NewspeakLexer
import io.github.bstrauss.newspeak.parser.NewspeakParserDefinition

/** Where TODO markers are searched: inside `(* ... *)`, not counting the delimiters. */
class NewspeakIndexPatternBuilder : IndexPatternBuilder {
    override fun getIndexingLexer(file: PsiFile): Lexer? = if (file is NewspeakFile) NewspeakLexer() else null
    override fun getCommentTokenSet(file: PsiFile): TokenSet? =
        if (file is NewspeakFile) NewspeakParserDefinition.COMMENTS else null
    override fun getCommentStartDelta(tokenType: IElementType?): Int = 2
    override fun getCommentEndDelta(tokenType: IElementType?): Int = 2
}
```

`NewspeakSpellcheckingStrategy.kt`:

```kotlin
package io.github.bstrauss.newspeak.editor

import com.intellij.psi.PsiComment
import com.intellij.psi.PsiElement
import com.intellij.spellchecker.tokenizer.SpellcheckingStrategy
import com.intellij.spellchecker.tokenizer.Tokenizer
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes

/** Spell checking in comments and strings, and nowhere else: identifiers are code. */
class NewspeakSpellcheckingStrategy : SpellcheckingStrategy() {
    override fun getTokenizer(element: PsiElement): Tokenizer<*> = when {
        element is PsiComment -> SpellcheckingStrategy.TEXT_TOKENIZER
        element.node?.elementType == NewspeakTokenTypes.STRING -> SpellcheckingStrategy.TEXT_TOKENIZER
        else -> SpellcheckingStrategy.EMPTY_TOKENIZER
    }
}
```

Before registering the spellchecking strategy, find out how the target platform ships the spellchecker: list the bundled plugins of the downloaded IDE (`find ~/.gradle/caches -maxdepth 6 -type d -path '*idea*2026.2.3*' -name plugins`, then `ls` it and look for a spellchecker directory; its `META-INF/plugin.xml` or `lib/*.jar!/META-INF/plugin.xml` gives the plugin ID).
- If it is a bundled plugin (ID `com.intellij.spellchecker` or similar): add `bundledPlugin("<that id>")` inside `intellijPlatform { }` in `dependencies`, and in `plugin.xml` an optional dependency `<depends optional="true" config-file="newspeak-spellchecker.xml"><that id></depends>`, with `src/main/resources/META-INF/newspeak-spellchecker.xml` holding `<idea-plugin><extensions defaultExtensionNs="com.intellij"><spellchecker.support language="Newspeak" implementationClass="io.github.bstrauss.newspeak.editor.NewspeakSpellcheckingStrategy"/></extensions></idea-plugin>`.
- If the classes are part of the core platform: register `<spellchecker.support language="Newspeak" implementationClass="io.github.bstrauss.newspeak.editor.NewspeakSpellcheckingStrategy"/>` directly in `plugin.xml`.

Record which one it was in the commit message.

- [ ] **Step 7: Register everything**

Inside `plugin.xml`'s `<extensions defaultExtensionNs="com.intellij">`, after the `fileType`:

```xml
        <lang.parserDefinition language="Newspeak"
                               implementationClass="io.github.bstrauss.newspeak.parser.NewspeakParserDefinition"/>
        <lang.syntaxHighlighterFactory language="Newspeak"
                                       implementationClass="io.github.bstrauss.newspeak.highlighting.NewspeakSyntaxHighlighterFactory"/>
        <colorSettingsPage implementation="io.github.bstrauss.newspeak.highlighting.NewspeakColorSettingsPage"/>
        <lang.braceMatcher language="Newspeak"
                           implementationClass="io.github.bstrauss.newspeak.editor.NewspeakBraceMatcher"/>
        <lang.commenter language="Newspeak"
                        implementationClass="io.github.bstrauss.newspeak.editor.NewspeakCommenter"/>
        <lang.quoteHandler language="Newspeak"
                           implementationClass="io.github.bstrauss.newspeak.editor.NewspeakQuoteHandler"/>
        <indexPatternBuilder implementation="io.github.bstrauss.newspeak.editor.NewspeakIndexPatternBuilder"/>
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains check`
Expected: `BUILD SUCCESSFUL`, all tests passing. If `testTodoInACommentIsFound` finds nothing, the TODO index needs a lexer-based indexer on this platform too: add `editor/NewspeakTodoIndexer.kt` extending `com.intellij.psi.impl.cache.impl.todo.LexerBasedTodoIndexer`, whose `createLexer(consumer)` answers a `com.intellij.psi.impl.cache.impl.BaseFilterLexer` over `NewspeakLexer()` that calls `advanceTodoItemCountsInToken()` for each token, and register it as `<todoIndexer filetype="Newspeak" implementationClass="…NewspeakTodoIndexer"/>`; then rerun.

- [ ] **Step 9: Commit**

```bash
git add editors/jetbrains
git commit -m "feat: highlighting, colour settings and lexer-based editor features"
```

---

### Task 5: The language server connection and the settings

**Files:**
- Create: `lsp/ServerBinaryLocator.kt`, `lsp/NewspeakLspIntegrationProvider.kt`, `lsp/NewspeakLspClientDescriptor.kt`, `lsp/NewspeakSemanticTokens.kt`, `lsp/MissingServerNotification.kt`, `lsp/RestartServerAction.kt`
- Create: `settings/NewspeakSettings.kt`, `settings/NewspeakSettingsConfigurable.kt`
- Modify: `src/main/resources/META-INF/plugin.xml`
- Create: `src/test/kotlin/io/github/bstrauss/newspeak/lsp/ServerBinaryLocatorTest.kt`, `lsp/NewspeakSemanticTokensTest.kt`

**Interfaces:**
- Consumes: `NewspeakColors` (Task 4); the server's legend from Part 1, Task 4 — token types `parameter`, `variable`, `property`, `class`, `method`; modifiers `declaration`, `readonly`.
- Produces:
  ```kotlin
  sealed interface ServerLocation {
      data class Found(val path: Path) : ServerLocation
      data class ConfiguredPathMissing(val path: String) : ServerLocation
      data class NotBundledForPlatform(val platform: String?) : ServerLocation
  }
  class ServerBinaryLocator(configuredPath: String?, pluginDirectory: Path?, osName: String, osArch: String) { fun locate(): ServerLocation }
  fun platformDirectory(osName: String, osArch: String): String?
  fun semanticAttributesFor(tokenType: String, modifiers: List<String>): TextAttributesKey?
  ```

- [ ] **Step 1: Confirm the LSP API against the platform**

The code below uses the 2026.1.4 names from the platform's LSP page (`plugins.jetbrains.com/docs/intellij/language-server-protocol.html`). Confirm each against the downloaded platform before writing it, since the page shows only a minimal sample. The LSP client API is open source since 2026.2, so Gradle attaches its sources; otherwise use `javap`:

```bash
LSP_JAR=$(find ~/.gradle/caches -path '*2026.2.3*' -name '*lsp*.jar' | head -1); echo "$LSP_JAR"
unzip -l "$LSP_JAR" | grep -E 'platform/lsp/api/(LspIntegrationProvider|ProjectWideLspClientDescriptor|LspClientDescriptor|LspClientManager|LspServerManager|customization/LspCustomization|customization/LspSemanticTokens)'
javap -cp "$LSP_JAR" com.intellij.platform.lsp.api.LspIntegrationProvider com.intellij.platform.lsp.api.ProjectWideLspClientDescriptor
```

Check, and adapt the code below to what the platform has, keeping the behaviour:
- `LspIntegrationProvider.fileOpened(project, file, clientStarter)` and `clientStarter.ensureClientStarted(descriptor)`;
- `ProjectWideLspClientDescriptor(project, presentableName)` with `isSupportedFile` and `createCommandLine`;
- `lspCustomization` on the descriptor, the `LspCustomization` class and its `semanticTokensCustomizer` property, and `LspSemanticTokensSupport.getTextAttributesKey(tokenType, modifiers)` (package `com.intellij.platform.lsp.api.customization`);
- the manager that restarts clients — before 2026.1.4 `LspServerManager.getInstance(project).stopAndRestartIfNeeded(providerClass)`; its 2026.1.4 name (likely `LspClientManager`).

Only the renamed, non-deprecated API is allowed (Global Constraints). If a name here is wrong, use the platform's and note it in the commit message.

- [ ] **Step 2: Write the failing tests**

`ServerBinaryLocatorTest.kt`:

```kotlin
package io.github.bstrauss.newspeak.lsp

import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.nio.file.Files
import java.nio.file.Path

class ServerBinaryLocatorTest {
    @get:Rule val temporary = TemporaryFolder()

    private fun pluginWithServer(platform: String, executable: String): Path {
        val plugin = temporary.newFolder("plugin").toPath()
        val binary = plugin.resolve("bin").resolve(platform).resolve(executable)
        Files.createDirectories(binary.parent)
        Files.writeString(binary, "")
        return plugin
    }

    @Test fun theConfiguredPathWins() {
        val plugin = pluginWithServer("windows-x64", "ns-next-lsp.exe")
        val configured = temporary.newFile("my-ns-next-lsp.exe").toPath()
        val located = ServerBinaryLocator(configured.toString(), plugin, "Windows 11", "amd64").locate()
        assertEquals(ServerLocation.Found(configured), located)
    }

    @Test fun aConfiguredPathThatDoesNotExistIsReportedNotSkipped() {
        val plugin = pluginWithServer("windows-x64", "ns-next-lsp.exe")
        val missing = temporary.root.toPath().resolve("absent.exe").toString()
        val located = ServerBinaryLocator(missing, plugin, "Windows 11", "amd64").locate()
        assertEquals(ServerLocation.ConfiguredPathMissing(missing), located)
    }

    @Test fun aBlankConfiguredPathFallsBackToTheBundledServer() {
        val plugin = pluginWithServer("windows-x64", "ns-next-lsp.exe")
        val located = ServerBinaryLocator("  ", plugin, "Windows 11", "amd64").locate()
        assertEquals(ServerLocation.Found(plugin.resolve("bin/windows-x64/ns-next-lsp.exe")), located)
    }

    @Test fun theBundledServerIsPerPlatform() {
        val plugin = pluginWithServer("linux-arm64", "ns-next-lsp")
        assertEquals(
            ServerLocation.Found(plugin.resolve("bin/linux-arm64/ns-next-lsp")),
            ServerBinaryLocator(null, plugin, "Linux", "aarch64").locate(),
        )
    }

    @Test fun noServerForThisPlatform() {
        val plugin = pluginWithServer("windows-x64", "ns-next-lsp.exe")
        assertEquals(
            ServerLocation.NotBundledForPlatform("macos-arm64"),
            ServerBinaryLocator(null, plugin, "Mac OS X", "aarch64").locate(),
        )
    }

    @Test fun anUnknownPlatformHasNoDirectory() {
        assertEquals(null, platformDirectory("Plan 9", "mips"))
        assertEquals(
            ServerLocation.NotBundledForPlatform(null),
            ServerBinaryLocator(null, temporary.root.toPath(), "Plan 9", "mips").locate(),
        )
    }

    @Test fun platformDirectories() {
        assertEquals("windows-x64", platformDirectory("Windows 11", "amd64"))
        assertEquals("windows-arm64", platformDirectory("Windows 11", "aarch64"))
        assertEquals("macos-x64", platformDirectory("Mac OS X", "x86_64"))
        assertEquals("macos-arm64", platformDirectory("Mac OS X", "aarch64"))
        assertEquals("linux-x64", platformDirectory("Linux", "amd64"))
        assertEquals("linux-arm64", platformDirectory("Linux", "arm64"))
    }
}
```

`NewspeakSemanticTokensTest.kt`:

```kotlin
package io.github.bstrauss.newspeak.lsp

import io.github.bstrauss.newspeak.highlighting.NewspeakColors
import org.junit.Assert.assertEquals
import org.junit.Test

/** The server's legend (`crates/ns-lsp/src/semantic_tokens.rs`, `legend`) onto the plugin's keys. */
class NewspeakSemanticTokensTest {
    @Test fun everyTokenTypeOfTheLegendIsMapped() {
        assertEquals(NewspeakColors.PARAMETER, semanticAttributesFor("parameter", listOf("declaration")))
        assertEquals(NewspeakColors.LOCAL_VARIABLE, semanticAttributesFor("variable", emptyList()))
        assertEquals(NewspeakColors.SLOT, semanticAttributesFor("property", emptyList()))
        assertEquals(NewspeakColors.IMMUTABLE_SLOT, semanticAttributesFor("property", listOf("readonly")))
        assertEquals(NewspeakColors.CLASS_NAME, semanticAttributesFor("class", emptyList()))
        assertEquals(NewspeakColors.METHOD_DECLARATION, semanticAttributesFor("method", listOf("declaration")))
        assertEquals(NewspeakColors.SEND, semanticAttributesFor("method", emptyList()))
    }

    @Test fun anUnknownTypeKeepsTheLexersColour() {
        assertEquals(null, semanticAttributesFor("macro", emptyList()))
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains check`
Expected: compilation fails — `Unresolved reference: ServerBinaryLocator`.

- [ ] **Step 4: The locator**

`ServerBinaryLocator.kt`:

```kotlin
package io.github.bstrauss.newspeak.lsp

import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.attribute.PosixFilePermission

/** Where the language server was found, or why it was not. */
sealed interface ServerLocation {
    data class Found(val path: Path) : ServerLocation
    /** A path is configured, and nothing is there: reported, never silently replaced by the bundled server. */
    data class ConfiguredPathMissing(val path: String) : ServerLocation
    /** No server is configured and none is bundled for [platform]; null when the platform is not one the layout names. */
    data class NotBundledForPlatform(val platform: String?) : ServerLocation
}

/**
 * The server to start, in the order `docs/design/editor-support.md` gives:
 * the configured path when one is set, else the one bundled for this platform
 * under `<plugin>/bin/<os>-<arch>/`. Pure over its inputs, so the order is
 * testable without an IDE.
 */
class ServerBinaryLocator(
    private val configuredPath: String?,
    private val pluginDirectory: Path?,
    private val osName: String,
    private val osArch: String,
) {
    fun locate(): ServerLocation {
        val configured = configuredPath?.trim().orEmpty()
        if (configured.isNotEmpty()) {
            val path = Path.of(configured)
            return if (Files.isRegularFile(path)) ServerLocation.Found(path) else ServerLocation.ConfiguredPathMissing(configured)
        }
        val platform = platformDirectory(osName, osArch)
        val bundled = if (platform == null || pluginDirectory == null) {
            null
        } else {
            pluginDirectory.resolve("bin").resolve(platform).resolve(executableName(osName))
        }
        if (bundled == null || !Files.isRegularFile(bundled)) return ServerLocation.NotBundledForPlatform(platform)
        ensureExecutable(bundled)
        return ServerLocation.Found(bundled)
    }

    /** The zip format the plugin ships in does not carry the executable bit. */
    private fun ensureExecutable(path: Path) {
        if (osName.startsWith("Windows") || Files.isExecutable(path)) return
        try {
            val permissions = Files.getPosixFilePermissions(path) +
                setOf(PosixFilePermission.OWNER_EXECUTE, PosixFilePermission.GROUP_EXECUTE, PosixFilePermission.OTHERS_EXECUTE)
            Files.setPosixFilePermissions(path, permissions)
        } catch (exception: UnsupportedOperationException) {
            // Not a POSIX file system: nothing to set.
        } catch (exception: java.io.IOException) {
            // Starting it will fail and report why; there is nothing better to do here.
        }
    }
}

/** `windows-x64`, `macos-arm64`, ...: the directory under `bin/` for this machine, or null for one the layout does not name. */
fun platformDirectory(osName: String, osArch: String): String? {
    val system = when {
        osName.startsWith("Windows") -> "windows"
        osName.startsWith("Mac") -> "macos"
        osName.startsWith("Linux") -> "linux"
        else -> return null
    }
    val architecture = when (osArch.lowercase()) {
        "amd64", "x86_64" -> "x64"
        "aarch64", "arm64" -> "arm64"
        else -> return null
    }
    return "$system-$architecture"
}

private fun executableName(osName: String): String =
    if (osName.startsWith("Windows")) "ns-next-lsp.exe" else "ns-next-lsp"
```

- [ ] **Step 5: Settings**

`settings/NewspeakSettings.kt`:

```kotlin
package io.github.bstrauss.newspeak.settings

import com.intellij.openapi.components.BaseState
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.SimplePersistentStateComponent
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.service

/** Application-level: the server path is specific to a machine and does not belong in `.idea/`. */
@Service(Service.Level.APP)
@State(name = "NewspeakSettings", storages = [Storage("newspeak.xml")])
class NewspeakSettings : SimplePersistentStateComponent<NewspeakSettings.Settings>(Settings()) {
    class Settings : BaseState() {
        /** A language server to start instead of the bundled one; null for the bundled one. */
        var serverPath by string()
    }

    companion object {
        fun getInstance(): NewspeakSettings = service()
    }
}
```

`settings/NewspeakSettingsConfigurable.kt`:

```kotlin
package io.github.bstrauss.newspeak.settings

import com.intellij.openapi.options.BoundConfigurable
import com.intellij.openapi.ui.DialogPanel
import com.intellij.ui.dsl.builder.AlignX
import com.intellij.ui.dsl.builder.bindText
import com.intellij.ui.dsl.builder.panel
import io.github.bstrauss.newspeak.lsp.restartNewspeakServers

class NewspeakSettingsConfigurable : BoundConfigurable("Newspeak") {
    override fun createPanel(): DialogPanel {
        val settings = NewspeakSettings.getInstance().state
        return panel {
            row("Language server:") {
                textField()
                    .bindText({ settings.serverPath.orEmpty() }, { text -> settings.serverPath = text.trim().ifEmpty { null } })
                    .align(AlignX.FILL)
                    .comment(
                        "Leave empty to use the ns-next-lsp bundled with the plugin. Point it at a locally built " +
                            "target/debug/ns-next-lsp to see changes to ns-next's parser or resolver at once."
                    )
            }
        }
    }

    override fun apply() {
        super.apply()
        restartNewspeakServers()
    }
}
```

- [ ] **Step 6: The semantic tokens, the notification, the provider, the descriptor, the restart**

`lsp/NewspeakSemanticTokens.kt`:

```kotlin
package io.github.bstrauss.newspeak.lsp

import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.platform.lsp.api.customization.LspSemanticTokensSupport
import io.github.bstrauss.newspeak.highlighting.NewspeakColors

/** The server's legend onto the plugin's keys; null leaves the lexer's colour in place. */
fun semanticAttributesFor(tokenType: String, modifiers: List<String>): TextAttributesKey? = when (tokenType) {
    "parameter" -> NewspeakColors.PARAMETER
    "variable" -> NewspeakColors.LOCAL_VARIABLE
    "property" -> if ("readonly" in modifiers) NewspeakColors.IMMUTABLE_SLOT else NewspeakColors.SLOT
    "class" -> NewspeakColors.CLASS_NAME
    "method" -> if ("declaration" in modifiers) NewspeakColors.METHOD_DECLARATION else NewspeakColors.SEND
    else -> null
}

object NewspeakSemanticTokens : LspSemanticTokensSupport() {
    override fun getTextAttributesKey(tokenType: String, modifiers: List<String>): TextAttributesKey? =
        semanticAttributesFor(tokenType, modifiers)
}
```

`lsp/MissingServerNotification.kt`:

```kotlin
package io.github.bstrauss.newspeak.lsp

import com.intellij.notification.NotificationAction
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import com.intellij.openapi.options.ShowSettingsUtil
import com.intellij.openapi.project.Project
import io.github.bstrauss.newspeak.settings.NewspeakSettingsConfigurable
import java.util.concurrent.atomic.AtomicBoolean

/** One balloon per IDE session: every `.ns` file opened after it would say the same. */
object MissingServerNotification {
    private val shown = AtomicBoolean(false)

    fun showOnce(project: Project, location: ServerLocation) {
        if (!shown.compareAndSet(false, true)) return
        val content = when (location) {
            is ServerLocation.ConfiguredPathMissing ->
                "The configured language server does not exist: ${location.path}"
            is ServerLocation.NotBundledForPlatform ->
                "No ns-next-lsp is bundled for ${location.platform ?: "this platform"}. Build it with " +
                    "<code>cargo build -p ns-lsp --release</code> and set its path."
            is ServerLocation.Found -> return
        }
        NotificationGroupManager.getInstance()
            .getNotificationGroup("Newspeak")
            .createNotification("Newspeak: errors and semantic colours are off", content, NotificationType.WARNING)
            .addAction(NotificationAction.createSimpleExpiring("Configure…") {
                ShowSettingsUtil.getInstance().showSettingsDialog(project, NewspeakSettingsConfigurable::class.java)
            })
            .notify(project)
    }

    /** After the setting changes, a new problem deserves a new balloon. */
    fun reset() = shown.set(false)
}
```

`lsp/NewspeakLspClientDescriptor.kt`:

```kotlin
package io.github.bstrauss.newspeak.lsp

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.ProjectWideLspClientDescriptor
import com.intellij.platform.lsp.api.customization.LspCustomization
import com.intellij.platform.lsp.api.customization.LspSemanticTokensCustomizer
import io.github.bstrauss.newspeak.NewspeakFileType
import java.nio.file.Path

class NewspeakLspClientDescriptor(project: Project, private val server: Path) :
    ProjectWideLspClientDescriptor(project, "Newspeak") {
    override fun isSupportedFile(file: VirtualFile): Boolean = file.fileType == NewspeakFileType

    override fun createCommandLine(): GeneralCommandLine = GeneralCommandLine(server.toString())

    override val lspCustomization: LspCustomization = object : LspCustomization() {
        override val semanticTokensCustomizer: LspSemanticTokensCustomizer = NewspeakSemanticTokens
    }
}
```

`lsp/NewspeakLspIntegrationProvider.kt`:

```kotlin
package io.github.bstrauss.newspeak.lsp

import com.intellij.ide.plugins.PluginManagerCore
import com.intellij.openapi.extensions.PluginId
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspClientStarter
import com.intellij.platform.lsp.api.LspIntegrationProvider
import io.github.bstrauss.newspeak.NewspeakFileType
import io.github.bstrauss.newspeak.PLUGIN_ID
import io.github.bstrauss.newspeak.settings.NewspeakSettings

class NewspeakLspIntegrationProvider : LspIntegrationProvider {
    override fun fileOpened(project: Project, file: VirtualFile, clientStarter: LspClientStarter) {
        if (file.fileType != NewspeakFileType) return
        when (val location = locateServer()) {
            is ServerLocation.Found -> clientStarter.ensureClientStarted(NewspeakLspClientDescriptor(project, location.path))
            else -> MissingServerNotification.showOnce(project, location)
        }
    }
}

fun locateServer(): ServerLocation = ServerBinaryLocator(
    configuredPath = NewspeakSettings.getInstance().state.serverPath,
    pluginDirectory = PluginManagerCore.getPlugin(PluginId.getId(PLUGIN_ID))?.pluginPath,
    osName = System.getProperty("os.name"),
    osArch = System.getProperty("os.arch"),
).locate()
```

`lsp/RestartServerAction.kt` — use the restart manager Step 1 found; with the pre-2026.1.4 shape renamed:

```kotlin
package io.github.bstrauss.newspeak.lsp

import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.project.DumbAwareAction
import com.intellij.openapi.project.ProjectManager
import com.intellij.platform.lsp.api.LspClientManager

/** Restarts the server in every open project, after the path setting changed or by hand. */
fun restartNewspeakServers() {
    MissingServerNotification.reset()
    for (project in ProjectManager.getInstance().openProjects) {
        LspClientManager.getInstance(project).stopAndRestartIfNeeded(NewspeakLspIntegrationProvider::class.java)
    }
}

class RestartServerAction : DumbAwareAction() {
    override fun actionPerformed(event: AnActionEvent) = restartNewspeakServers()
}
```

- [ ] **Step 7: Register them**

In `plugin.xml`'s `<extensions defaultExtensionNs="com.intellij">`:

```xml
        <applicationConfigurable parentId="language"
                                 instance="io.github.bstrauss.newspeak.settings.NewspeakSettingsConfigurable"
                                 id="io.github.bstrauss.newspeak.settings"
                                 displayName="Newspeak"/>
        <notificationGroup id="Newspeak" displayType="BALLOON"/>
        <platform.lsp.integrationProvider implementation="io.github.bstrauss.newspeak.lsp.NewspeakLspIntegrationProvider"/>
```

After `</extensions>`:

```xml
    <actions>
        <action id="io.github.bstrauss.newspeak.RestartServer"
                class="io.github.bstrauss.newspeak.lsp.RestartServerAction"
                text="Restart Newspeak Language Server"
                description="Stops the Newspeak language server and starts it again">
            <add-to-group group-id="ToolsMenu" anchor="last"/>
        </action>
    </actions>
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains check`
Expected: `BUILD SUCCESSFUL`, all tests passing, including `verifyPluginProjectConfiguration` (part of `check`) with no warning about the LSP dependency.

- [ ] **Step 9: Commit**

```bash
git add editors/jetbrains
git commit -m "feat: connect the plugin to ns-next-lsp over the native LSP API"
```

---

### Task 6: Packaging the server, notices and Marketplace configuration

**Files:**
- Modify: `editors/jetbrains/build.gradle.kts`
- Create: `editors/jetbrains/THIRD-PARTY-NOTICES.txt`

- [ ] **Step 1: Bundle the release server and the notices**

Append to `build.gradle.kts`:

```kotlin
/** The platform this build runs on, as `ServerBinaryLocator` names it; only its server is bundled for now. */
val hostPlatform: String = run {
    val osName = System.getProperty("os.name")
    val system = when {
        osName.startsWith("Windows") -> "windows"
        osName.startsWith("Mac") -> "macos"
        else -> "linux"
    }
    val architecture = when (System.getProperty("os.arch").lowercase()) {
        "aarch64", "arm64" -> "arm64"
        else -> "x64"
    }
    "$system-$architecture"
}

val buildLanguageServerRelease by tasks.registering(Exec::class) {
    description = "Builds ns-next-lsp (release) with Cargo for the plugin distribution."
    workingDir = repositoryRoot
    commandLine("cargo", "build", "-p", "ns-lsp", "--release")
}

tasks.prepareSandbox {
    dependsOn(buildLanguageServerRelease)
    // Beside the jars rather than inside one, so it starts without being extracted.
    from(repositoryRoot.resolve("target/release/ns-next-lsp$executableSuffix")) {
        into(pluginName.map { name -> "$name/bin/$hostPlatform" })
    }
    from(layout.projectDirectory.file("THIRD-PARTY-NOTICES.txt")) {
        into(pluginName)
    }
    from(repositoryRoot.resolve("LICENSE")) {
        into(pluginName)
    }
}

intellijPlatform {
    signing {
        certificateChain = providers.environmentVariable("NEWSPEAK_PLUGIN_CERTIFICATE_CHAIN")
        privateKey = providers.environmentVariable("NEWSPEAK_PLUGIN_PRIVATE_KEY")
        password = providers.environmentVariable("NEWSPEAK_PLUGIN_PRIVATE_KEY_PASSWORD")
    }
    publishing {
        token = providers.environmentVariable("NEWSPEAK_PLUGIN_PUBLISH_TOKEN")
    }
}
```

`prepareSandbox` also feeds `runIde`, so the sandbox IDE starts the release server. If `tasks.prepareSandbox` does not expose `pluginName` in this plugin version, use `intellijPlatform.projectName` — check with `./editors/jetbrains/gradlew -p editors/jetbrains help --task prepareSandbox`.

- [ ] **Step 2: Write the notices**

List the crates compiled into the binary:

```bash
cargo tree -p ns-lsp -e normal --prefix none --format '{p} {l}' | sort -u
```

Write `editors/jetbrains/THIRD-PARTY-NOTICES.txt`: a header line (`ns-next-lsp, bundled in this plugin, contains the following third-party crates.`), then one line per crate from that output except `ns-lsp`, `ns-runtime` and `ns-syntax` (this repository, MIT), giving name, version and licence expression, then for each distinct licence the full licence text once (MIT and Apache-2.0 — copy them from any of those crates' `LICENSE-MIT` / `LICENSE-APACHE` under `~/.cargo/registry/src/*/`), and for every crate whose licence is MIT the copyright line from its own `LICENSE-MIT` (the MIT licence requires each copyright notice). If `cargo tree` shows a licence other than MIT or Apache-2.0, stop and report it to the controller: that is a licensing decision.

- [ ] **Step 3: Build the distribution and look inside**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains buildPlugin`
Then: `unzip -l editors/jetbrains/build/distributions/*.zip`
Expected: the zip holds `Newspeak (ns-next)/lib/*.jar` (or the directory name the build uses), `…/bin/windows-x64/ns-next-lsp.exe`, `…/THIRD-PARTY-NOTICES.txt` and `…/LICENSE`.

- [ ] **Step 4: Try it by hand**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains runIde`
In the sandbox IDE open the repository root as a project and open `goldens/errors/` modules and `crates/ns-runtime/core-library/Kernel.ns`. Check: the file icon; colours before the server answers; errors underlined with ns-next's message in `goldens/errors` files that fail before running (a resolver error); parameters, slots and nested classes coloured differently after a moment; *Settings | Editor | Color Scheme | Newspeak* shows the demo; *Tools | Restart Newspeak Language Server* works; a bogus path in *Settings | Languages & Frameworks | Newspeak* produces the balloon once and leaves highlighting intact. Report what you saw to the controller; this is not automated.

- [ ] **Step 5: Commit**

```bash
git add editors/jetbrains
git commit -m "feat: bundle ns-next-lsp and third-party notices into the plugin"
```

---

### Task 7: Plugin verification, documentation and the end-of-batch gates

**Files:**
- Modify: `editors/jetbrains/build.gradle.kts`
- Modify: `CLAUDE.md`, `docs/design/README.md`, `docs/design/editor-support.md`, `docs/PROGRESS.md`

- [ ] **Step 1: Configure the verifier**

List what the verifier could select:

```bash
./editors/jetbrains/gradlew -p editors/jetbrains printProductsReleases
```

Then configure, inside the existing `intellijPlatform { }` block:

```kotlin
    pluginVerification {
        ides {
            // The lowest supported and the current release of four commercial
            // IDEs: enough to catch an API one of them lacks, without
            // downloading every patch release.
            create(IntelliJPlatformType.IntellijIdea, "2026.1.4")
            create(IntelliJPlatformType.IntellijIdea, providers.gradleProperty("platformVersion"))
            create(IntelliJPlatformType.RustRover, "<lowest 2026.1.x release at or after build 261.26222>")
            create(IntelliJPlatformType.RustRover, "<current 2026.2.x release>")
            create(IntelliJPlatformType.PyCharm, "<lowest 2026.1.x release at or after build 261.26222>")
            create(IntelliJPlatformType.CLion, "<current 2026.2.x release>")
        }
    }
```

with `import org.jetbrains.intellij.platform.gradle.IntelliJPlatformType` at the top. Fill each placeholder with a version `printProductsReleases` printed — these are values to read off, not to guess. If the plugin's version of the DSL spells `create` differently, follow `./editors/jetbrains/gradlew -p editors/jetbrains help --task verifyPlugin`.

- [ ] **Step 2: Run the verifier**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains verifyPlugin`
Expected: `BUILD SUCCESSFUL`; the report lists every IDE as `Compatible`, with no deprecated or internal API usage. A deprecated-API report is a defect to fix, not to accept (Global Constraints).

- [ ] **Step 3: Update the documentation**

- `CLAUDE.md`: in the opening, after the `ns-lsp` bullet Part 1 added, a paragraph: `editors/jetbrains/` — the JetBrains plugin (Kotlin, Gradle wrapper inside): a lexer mirroring `ns-syntax`'s, compared against it token by token over every module (`LexerComparisonTest`), and the connection to `ns-next-lsp`. In "Working in this repository", add the plugin's gates to the blocks:

  ```bash
  # --- before every commit that touches editors/jetbrains, crates/ns-lsp or crates/ns-syntax ---
  ./editors/jetbrains/gradlew -p editors/jetbrains check

  # --- once at the end of a batch of work (downloads several IDEs) ---
  ./editors/jetbrains/gradlew -p editors/jetbrains verifyPlugin buildPlugin
  ```

  and one sentence: a change to `crates/ns-syntax/src/lexer.rs` must be mirrored in `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/lexer/NewspeakLexer.kt` in the same commit, or the comparison fails.
- `docs/design/editor-support.md`: status **Built** (highlighting and errors as you type; formatting, navigation and further platforms planned). Every remaining `Planned:` section about the plugin, its build and its tests becomes present tense, describing what exists, and records what implementation settled: how the spellchecker is wired (Task 4, Step 6), the LSP API names confirmed in Task 5, Step 1, the verifier's IDE list, whether a `todoIndexer` was needed. Keep "Planned later" as it is.
- `docs/design/README.md`: the map row's status becomes `Built — highlighting and errors as you type in every commercial JetBrains IDE from 2026.1.4; the server bundled for Windows x64`, planned next `formatting, navigation, more platforms`; mention `editors/jetbrains/` in "Architecture" and the plugin gates in "Gates".
- `docs/PROGRESS.md`: a milestone entry in the file's existing style: editor support, what was built, with the date, and — if "Where the next session starts" should mention it — a line there.

- [ ] **Step 4: Run every gate**

In Git Bash, from the repository root, in the foreground:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace --exclude ns-lsp
cargo test -p ns-lsp
cargo run -p ns-cli -- parse-corpus corpus/modules      # parsed 210, failed 0, expected-fail 1, stale 0
cargo run -p ns-cli -- run-corpus corpus/modules        # ran 6, failed 0, skipped 205
./editors/jetbrains/gradlew -p editors/jetbrains check
./editors/jetbrains/gradlew -p editors/jetbrains verifyPlugin buildPlugin
```

(The runtime's stress modes ran at the end of Part 1 and nothing under `crates/ns-runtime` changed since; if anything there did, run them too.) Expected: all green and both corpus reports exactly as shown. Report the output.

- [ ] **Step 5: Commit**

```bash
git add editors/jetbrains/build.gradle.kts CLAUDE.md docs
git commit -m "docs: the JetBrains plugin in the design documents and the gates"
```
