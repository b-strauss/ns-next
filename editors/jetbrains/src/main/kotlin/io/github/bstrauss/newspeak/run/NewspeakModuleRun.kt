package io.github.bstrauss.newspeak.run

import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.SymbolKind

/**
 * `main:args:` of a module ns-next can run from its source, or null: the
 * root class -- the answer's one top-level symbol -- is instantiated with
 * `new`, its factory written or omitted, and `main:args:` must be a public
 * instance-side method, since ns-next sends it to the instance as a public
 * send (`crates/ns-cli/src/program.rs`, `instantiate_application`). A
 * project's main module, built with `packageUsing:`, is run from its
 * `newspeak.json` instead.
 */
fun runnableMain(symbols: List<DocumentSymbol>): DocumentSymbol? {
    val members = symbols.firstOrNull()?.children.orEmpty()
    val builtWithNew = members.any { member -> member.kind == SymbolKind.Constructor && member.name == "new" }
    if (!builtWithNew) return null
    // A `detail` of `public` alone: `public class` is a class-side method, which ns-next never sends.
    return members.firstOrNull { member ->
        member.kind == SymbolKind.Method && selectorOf(member.name) == MAIN_SELECTOR && member.detail == "public"
    }
}

private const val MAIN_SELECTOR = "main:args:"

/**
 * A method's selector from its symbol's name, which the server writes as the
 * pattern (`main: platform args: arguments` → `main:args:`): its words ending
 * in `:`, joined. A unary or binary pattern has none, and answers "".
 */
private fun selectorOf(pattern: String): String =
    pattern.split(' ').filter { word -> word.endsWith(":") }.joinToString("")
