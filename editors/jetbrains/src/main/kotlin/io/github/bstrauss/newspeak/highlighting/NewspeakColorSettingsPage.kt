package io.github.bstrauss.newspeak.highlighting

import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.fileTypes.SyntaxHighlighter
import com.intellij.openapi.options.colors.AttributesDescriptor
import com.intellij.openapi.options.colors.ColorDescriptor
import com.intellij.openapi.options.colors.ColorSettingsPage
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.NewspeakIcons
import javax.swing.Icon

class NewspeakColorSettingsPage : ColorSettingsPage {
    override fun getDisplayName(): String = NewspeakBundle.message("color.settings.display.name")
    override fun getIcon(): Icon = NewspeakIcons.FILE
    override fun getHighlighter(): SyntaxHighlighter = NewspeakSyntaxHighlighter()
    override fun getColorDescriptors(): Array<ColorDescriptor> = ColorDescriptor.EMPTY_ARRAY

    override fun getAttributeDescriptors(): Array<AttributesDescriptor> = arrayOf(
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.keywords.keyword"), NewspeakColors.KEYWORD),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.keywords.reserved"), NewspeakColors.RESERVED_WORD),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.keywords.return"), NewspeakColors.RETURN),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.identifier"), NewspeakColors.IDENTIFIER),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.selectors.keyword"), NewspeakColors.KEYWORD_SELECTOR),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.selectors.setter"), NewspeakColors.SETTER_KEYWORD),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.selectors.binary"), NewspeakColors.BINARY_SELECTOR),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.selectors.angle.brackets"), NewspeakColors.ANGLE_BRACKET),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.operators.assignment"), NewspeakColors.ASSIGNMENT),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.literals.number"), NewspeakColors.NUMBER),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.literals.string"), NewspeakColors.STRING),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.literals.symbol"), NewspeakColors.SYMBOL),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.comment"), NewspeakColors.COMMENT),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.braces.parentheses"), NewspeakColors.PARENTHESES),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.braces.brackets"), NewspeakColors.BRACKETS),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.braces.braces"), NewspeakColors.BRACES),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.braces.slot.bars"), NewspeakColors.SLOT_BARS),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.punctuation.dot"), NewspeakColors.DOT),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.punctuation.semicolon"), NewspeakColors.SEMICOLON),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.bad.character"), NewspeakColors.BAD_CHARACTER),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.semantic.parameter"), NewspeakColors.PARAMETER),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.semantic.local.variable"), NewspeakColors.LOCAL_VARIABLE),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.semantic.slot"), NewspeakColors.SLOT),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.semantic.immutable.slot"), NewspeakColors.IMMUTABLE_SLOT),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.semantic.class"), NewspeakColors.CLASS_NAME),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.semantic.method.declaration"), NewspeakColors.METHOD_DECLARATION),
        AttributesDescriptor(NewspeakBundle.lazyMessage("color.settings.semantic.send"), NewspeakColors.SEND),
    )

    override fun getAdditionalHighlightingTagToDescriptorMap(): Map<String, TextAttributesKey> = mapOf(
        "parameter" to NewspeakColors.PARAMETER,
        "local" to NewspeakColors.LOCAL_VARIABLE,
        "slot" to NewspeakColors.SLOT,
        "immutable" to NewspeakColors.IMMUTABLE_SLOT,
        "class" to NewspeakColors.CLASS_NAME,
        "declaration" to NewspeakColors.METHOD_DECLARATION,
        "send" to NewspeakColors.SEND,
        "lazy" to NewspeakColors.KEYWORD,
    )

    override fun getDemoText(): String = """
        class <class>Counter</class> usingPlatform: <parameter>platform</parameter> = Object <send>new</send> (
          /// A counter. TODO: reset
          | private <slot>count</slot> ::= 0. private <immutable>limit</immutable> = 16rFF. |
        ) (
          public <declaration>increment:</declaration> <parameter>step</parameter> = (
            | <local>next</local> |
            <local>next::</local> <slot>count</slot> <send>+</send> <parameter>step</parameter>.
            <local>next</local> <send>></send> <immutable>limit</immutable> <send>ifTrue:</send> [^nil].
            <slot>count::</slot> <local>next</local>.
            ^self <send>report:</send> #counted <send>with:</send> 'done' <send>and:</send> "!"
          )
          <lazy>lazy</lazy> public <immutable>total</immutable> = <slot>count</slot> <send>*</send> 2.
          private <lazy>lazy</lazy> <slot>cache</slot> ::= <immutable>limit</immutable> <send>printString</send>.
          public <declaration>report:</declaration> <parameter>what</parameter> with: <parameter>text</parameter> and: <parameter>mark</parameter> = ( ^{ <parameter>what</parameter>. <parameter>text</parameter>. <parameter>mark</parameter> } )
        ) : ()
    """.trimIndent()
}
