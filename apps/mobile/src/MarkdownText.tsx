import { Fragment, memo, useMemo } from "react";
import type { ReactNode } from "react";
import { Linking, ScrollView, Text, View } from "react-native";
import { safeHttpUrl } from "@socium/client/links";
import { parseMarkdown } from "./markdown";
import type { Block, Inline } from "./markdown";
import { markdownStyles as styles } from "./styles";

function openLink(href: string) {
  // Checked again where the URL leaves the app.
  const url = safeHttpUrl(href);
  if (url) void Linking.openURL(url).catch(() => {});
}

function renderInline(inlines: Inline[]): ReactNode[] {
  return inlines.map((inline, index) => {
    switch (inline.type) {
      case "text":
        return <Fragment key={index}>{inline.text}</Fragment>;
      case "strong":
        return <Text key={index} style={styles.strong}>{renderInline(inline.children)}</Text>;
      case "emphasis":
        return <Text key={index} style={styles.emphasis}>{renderInline(inline.children)}</Text>;
      case "code":
        return <Text key={index} style={styles.inlineCode}>{inline.text}</Text>;
      case "link": {
        const { href } = inline;
        if (!href) return <Fragment key={index}>{renderInline(inline.children)}</Fragment>;
        return (
          <Text key={index} style={styles.link} accessibilityRole="link" accessibilityHint={href} onPress={() => openLink(href)}>
            {renderInline(inline.children)}
          </Text>
        );
      }
    }
  });
}

function MarkdownBlock({ block }: { block: Block }) {
  switch (block.type) {
    case "heading":
      return (
        <Text selectable accessibilityRole="header" style={[styles.heading, block.level <= 2 ? styles.headingLarge : styles.headingSmall]}>
          {renderInline(block.children)}
        </Text>
      );
    case "paragraph":
      return <Text selectable style={styles.paragraph}>{renderInline(block.children)}</Text>;
    case "quote":
      return (
        <View style={styles.quote}>
          <Text selectable style={[styles.paragraph, styles.quoteText]}>{renderInline(block.children)}</Text>
        </View>
      );
    case "rule":
      return <View style={styles.rule} />;
    case "code":
      return (
        <View style={styles.codeBlock}>
          {block.language ? <Text style={styles.codeLanguage}>{block.language}</Text> : null}
          <ScrollView horizontal nestedScrollEnabled showsHorizontalScrollIndicator={false}>
            <Text selectable style={styles.code}>{block.text}</Text>
          </ScrollView>
        </View>
      );
    case "list":
      return (
        <View style={styles.list}>
          {block.items.map((item, index) => {
            const marker = item.depth > 0 ? "◦" : item.number !== null ? `${item.number}.` : "•";
            return (
              <View key={index} style={[styles.listItem, { paddingLeft: item.depth * 18 }]}>
                <Text style={styles.listMarker}>{marker}</Text>
                <Text selectable style={[styles.paragraph, styles.listText]}>{renderInline(item.children)}</Text>
              </View>
            );
          })}
        </View>
      );
  }
}

/** An assistant reply as Markdown; links open in the browser only for http and https. */
export const MarkdownText = memo(function MarkdownText({ text }: { text: string }) {
  const blocks = useMemo(() => parseMarkdown(text), [text]);
  return (
    <View style={styles.document}>
      {blocks.map((block, index) => <MarkdownBlock key={index} block={block} />)}
    </View>
  );
});
