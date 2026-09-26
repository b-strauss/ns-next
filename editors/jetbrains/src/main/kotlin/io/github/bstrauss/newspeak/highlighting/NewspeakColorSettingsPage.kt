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
        AttributesDescriptor("Selectors//Setter (name::)", NewspeakColors.SETTER_KEYWORD),
        AttributesDescriptor("Selectors//Binary selector", NewspeakColors.BINARY_SELECTOR),
        AttributesDescriptor("Selectors//Angle brackets (< and >)", NewspeakColors.ANGLE_BRACKET),
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
        class <class>Counter</class> usingPlatform: <parameter>platform</parameter> = Object <send>new</send> (
          (* A counter. TODO: reset *)
          | private <slot>count</slot> ::= 0. private <immutable>limit</immutable> = 16rFF. |
        ) (
          public <declaration>increment:</declaration> <parameter>step</parameter> = (
            | <local>next</local> |
            <local>next::</local> <slot>count</slot> <send>+</send> <parameter>step</parameter>.
            <local>next</local> <send>></send> <immutable>limit</immutable> <send>ifTrue:</send> [^nil].
            <slot>count::</slot> <local>next</local>.
            ^self <send>report:</send> #counted <send>with:</send> 'done' <send>and:</send> "!"
          )
          public <declaration>report:</declaration> <parameter>what</parameter> with: <parameter>text</parameter> and: <parameter>mark</parameter> = ( ^{ <parameter>what</parameter>. <parameter>text</parameter>. <parameter>mark</parameter> } )
        ) : ()
    """.trimIndent()
}
