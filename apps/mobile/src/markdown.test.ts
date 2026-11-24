import { strict as assert } from "node:assert";
import { performance } from "node:perf_hooks";
import { test } from "node:test";
import { safeHttpUrl as safeLinkUrl } from "@socium/client/links";
import {
  maxInlineDepth, maxMarkdownNodes, maxRenderedLength, parseInline, parseMarkdown, truncationMarker,
} from "./markdown.ts";
import type { Inline } from "./markdown.ts";

const text = (value: string): Inline => ({ type: "text", text: value });

test("blocks: headings, paragraphs, rules, quotes, and fenced code", () => {
  assert.deepEqual(parseMarkdown([
    "# Title #",
    "First line",
    "same paragraph  ",
    "after a break",
    "",
    "---",
    "> quoted",
    "> more",
    "```ts",
    "const a = 1;",
    "",
    "# not a heading",
    "```",
    "###### Six",
  ].join("\n")), [
    { type: "heading", level: 1, children: [text("Title")] },
    { type: "paragraph", children: [text("First line same paragraph\nafter a break")] },
    { type: "rule" },
    { type: "quote", children: [text("quoted more")] },
    { type: "code", language: "ts", text: "const a = 1;\n\n# not a heading" },
    { type: "heading", level: 6, children: [text("Six")] },
  ]);
});

test("an unclosed fence, as in a reply still streaming, runs to the end", () => {
  assert.deepEqual(parseMarkdown("Look:\n~~~~\nfn main() {}\n```"), [
    { type: "paragraph", children: [text("Look:")] },
    { type: "code", language: "", text: "fn main() {}\n```" },
  ]);
});

test("lists: markers, numbering, nesting, continuation, and loose items", () => {
  assert.deepEqual(parseMarkdown([
    "Steps:",
    "3. third",
    "   continued",
    "",
    "4. fourth",
    "   - nested",
    "- other list",
    "* same list",
    "",
    "After.",
  ].join("\n")), [
    { type: "paragraph", children: [text("Steps:")] },
    { type: "list", ordered: true, start: 3, items: [
      { depth: 0, number: 3, children: [text("third continued")] },
      { depth: 0, number: 4, children: [text("fourth")] },
      { depth: 1, number: null, children: [text("nested")] },
    ] },
    { type: "list", ordered: false, start: 1, items: [
      { depth: 0, number: null, children: [text("other list")] },
      { depth: 0, number: null, children: [text("same list")] },
    ] },
    { type: "paragraph", children: [text("After.")] },
  ]);
});

test("inline: bold, italic, code, and escapes", () => {
  assert.deepEqual(parseInline("**bold** and *it* and __b__ and _i_ and ***both***"), [
    { type: "strong", children: [text("bold")] },
    text(" and "),
    { type: "emphasis", children: [text("it")] },
    text(" and "),
    { type: "strong", children: [text("b")] },
    text(" and "),
    { type: "emphasis", children: [text("i")] },
    text(" and "),
    { type: "strong", children: [{ type: "emphasis", children: [text("both")] }] },
  ]);
  assert.deepEqual(parseInline("**a *b* c**"), [
    { type: "strong", children: [text("a "), { type: "emphasis", children: [text("b")] }, text(" c")] },
  ]);
  assert.deepEqual(parseInline("run `cargo **test**` or `` a`b ``"), [
    text("run "),
    { type: "code", text: "cargo **test**" },
    text(" or "),
    { type: "code", text: "a`b" },
  ]);
  assert.deepEqual(parseInline("snake_case_name, 2 * 3 * 4, \\*literal\\*, `open"), [
    text("snake_case_name, 2 * 3 * 4, *literal*, `open"),
  ]);
});

test("links: markdown, autolinks, and bare URLs", () => {
  assert.deepEqual(parseInline("See [the **docs**](https://example.com/a_(b) \"Title\") now"), [
    text("See "),
    { type: "link", href: "https://example.com/a_(b)", children: [text("the "), { type: "strong", children: [text("docs")] }] },
    text(" now"),
  ]);
  assert.deepEqual(parseInline("<http://example.com/x> and https://example.com/path)."), [
    { type: "link", href: "http://example.com/x", children: [text("http://example.com/x")] },
    text(" and "),
    { type: "link", href: "https://example.com/path", children: [text("https://example.com/path")] },
    text(")."),
  ]);
  assert.deepEqual(parseInline("(see https://en.wikipedia.org/wiki/A_(b))"), [
    text("(see "),
    { type: "link", href: "https://en.wikipedia.org/wiki/A_(b)", children: [text("https://en.wikipedia.org/wiki/A_(b)")] },
    text(")"),
  ]);
});

test("links to anything but http and https keep their label and lose their target", () => {
  for (const target of [
    "javascript:alert(1)",
    "JaVaScRiPt:alert(1)",
    "file:///etc/passwd",
    "tel:+1555",
    "mailto:a@example.com",
    "data:text/html,<script>",
    "/relative/path",
    "//example.com/protocol-relative",
    "https:example.com",
    "java\tscript:alert(1)",
    "\u0000javascript:alert(1)",
    "https://exa mple.com",
    "intent://scan#Intent;scheme=zxing;end",
  ]) {
    assert.equal(safeLinkUrl(target), null, JSON.stringify(target));
    const [link] = parseInline(`[label](${target.replace(/\s/g, "%20")})`);
    if (link?.type === "link") assert.equal(link.href, null, JSON.stringify(target));
  }
  assert.deepEqual(parseInline("[click](javascript:alert(1))"), [
    { type: "link", href: null, children: [text("click")] },
  ]);
  assert.equal(safeLinkUrl(" HTTPS://Example.com/a?b=c#d "), "https://example.com/a?b=c#d");
  assert.equal(safeLinkUrl("http://localhost:4173/"), "http://localhost:4173/");
});

/** Parses `source` and fails if it takes long enough to show quadratic work or backtracking. */
function parseQuickly(source: string, limit = 100) {
  const started = performance.now();
  const blocks = parseMarkdown(source);
  const elapsed = performance.now() - started;
  assert(elapsed < limit, `parsing ${source.length} characters took ${elapsed.toFixed(1)} ms`);
  return blocks;
}

/** Every text in `blocks`, in order, to check nothing of a reply is lost. */
function allText(blocks: ReturnType<typeof parseMarkdown>) {
  const inline = (inlines: ReturnType<typeof parseInline>): string =>
    inlines.map((node) => node.type === "text" || node.type === "code" ? node.text : inline(node.children)).join("");
  return blocks.map((block) =>
    block.type === "code" ? block.text
      : block.type === "rule" ? ""
        : block.type === "list" ? block.items.map((item) => inline(item.children)).join("")
          : inline(block.children)).join("");
}

test("long unclosed delimiters parse in linear time", () => {
  const length = 40_000;
  for (const [name, source] of [
    ["brackets", "[".repeat(length)],
    ["brackets with text", "[a ".repeat(length / 3)],
    ["link openers", "[a](".repeat(length / 4)],
    ["asterisks", "*a ".repeat(length / 3)],
    ["underscores", "_a ".repeat(length / 3)],
    ["double asterisks", "**a ".repeat(length / 4)],
    ["backticks", "``a`".repeat(length / 4)],
    ["autolinks", "<http://a".repeat(length / 9)],
    ["parens in a bare URL", `https://example.com/${")".repeat(length)}`],
    ["escapes", "\\[".repeat(length / 3)],
  ] as const) {
    const blocks = parseQuickly(source);
    assert(allText(blocks).length > 0, name);
  }
});

test("deep nesting is capped instead of recursing without bound", () => {
  const levels = 20_000;
  const links = parseQuickly(`${"[".repeat(levels)}x${"](https://example.com)".repeat(levels)}`);
  const emphasis = parseQuickly(`${"*_".repeat(levels)}x${"_*".repeat(levels)}`);
  const strong = parseQuickly(`${"**a ".repeat(levels)}x${" a**".repeat(levels)}`);
  for (const blocks of [links, emphasis, strong]) {
    let depth = 0;
    const deepest = (inlines: ReturnType<typeof parseInline>, level: number) => {
      depth = Math.max(depth, level);
      for (const node of inlines) if ("children" in node) deepest(node.children, level + 1);
    };
    for (const block of blocks) if ("children" in block) deepest(block.children, 0);
    assert(depth <= maxInlineDepth, `nested ${depth} deep`);
  }
  // Past the cap, the text shows as written.
  const u = "https://example.com/";
  const link = (...children: ReturnType<typeof parseInline>) => ({ type: "link" as const, href: u, children });
  assert.deepEqual(parseInline(`[a [b [c [d [e](${u}) f](${u}) g](${u}) h](${u}) i](${u})`), [
    link(text("a "), link(text("b "), link(text("c "), link(text(`d [e](${u}) f`)), text(" g")), text(" h")), text(" i")),
  ]);
});

test("long runs of whitespace inside lines parse in linear time", () => {
  const spaces = " ".repeat(40_000);
  for (const source of [
    `a${spaces}b\nnext`,
    `a${spaces}b${spaces}\nnext`,
    `# a${spaces}b`,
    `# a${spaces}#x`,
    `\`\`\`a${spaces}\``,
    `- - -${spaces}x`,
    `-${spaces}x`,
    `> a${spaces}b`,
    `1.${spaces}a`,
    `\`${spaces}x${spaces}\``,
  ]) {
    assert(allText(parseQuickly(source)).includes("a") || source.includes("x"));
  }
  assert.deepEqual(parseQuickly(`# a${spaces}b #`), [{ type: "heading", level: 1, children: [text(`a${spaces}b`)] }]);
});

test("a long ordered list is numbered once while parsing", () => {
  const count = 500;
  const source = Array.from({ length: count }, (_, index) => `${index + 1}. item ${index + 1}${index % 50 ? "" : "\n   - nested"}`);
  const blocks = parseQuickly(source.join("\n"), 50);
  assert.equal(blocks.length, 1);
  const [list] = blocks;
  assert(list?.type === "list");
  const numbers = list.items.filter(({ depth }) => depth === 0).map(({ number }) => number);
  assert.deepEqual(numbers, Array.from({ length: count }, (_, index) => index + 1));
  assert(list.items.filter(({ depth }) => depth > 0).every(({ number }) => number === null));
  // Many blank-separated items do not rescan the lines after each one.
  parseQuickly(Array.from({ length: 5000 }, () => "- item").join("\n\n\n"));
});

test("a reply past the block limit still shows in full, as plain text", () => {
  const many = Array.from({ length: maxMarkdownNodes * 2 }, (_, index) => `# heading ${index}`).join("\n");
  const capped = parseQuickly(many);
  assert.equal(capped.length, maxMarkdownNodes + 1);
  assert.deepEqual(capped.at(-1), { type: "paragraph", children: [text(Array.from({ length: maxMarkdownNodes }, (_, index) => `# heading ${index + maxMarkdownNodes}`).join("\n"))] });

  const items = parseQuickly(Array.from({ length: maxMarkdownNodes * 2 }, (_, index) => `- item ${index}`).join("\n"));
  const [list, rest] = items;
  assert(list?.type === "list");
  assert.equal(list.items.length, maxMarkdownNodes - 1);
  assert.equal(rest?.type === "paragraph" && rest.children[0]?.type === "text" && rest.children[0].text.split("\n")[0], `- item ${maxMarkdownNodes - 1}`);
});

test("many blank lines inside a list before a deeply indented item parse in linear time", () => {
  const indent = " ".repeat(20_000);
  // Each source stays under the rendered cap, so all of it is parsed.
  for (const [blank, count] of [["", 20_000], ["   ", 5000]] as const) {
    const blocks = parseQuickly(`- a\n${`${blank}\n`.repeat(count)}${indent}- b`);
    const [list] = blocks;
    assert(list?.type === "list");
    assert.deepEqual(list.items.map(({ children }) => children), [[text("a")], [text("b")]]);
  }
});

test("long delimiter runs parse in linear time and still match", () => {
  for (const character of ["`", "*", "_"]) {
    const run = character.repeat(40_000);
    assert.equal(allText(parseQuickly(`${run}x${run}`)).includes("x"), true, character);
  }
  // Many closed spans, each found by moving forward from the last.
  const spans = parseQuickly("*a* `b` **c** ``d`` ".repeat(2000));
  const [paragraph] = spans;
  assert(paragraph?.type === "paragraph");
  assert.equal(paragraph.children.filter(({ type }) => type === "emphasis").length, 2000);
  assert.equal(paragraph.children.filter(({ type }) => type === "strong").length, 2000);
  assert.equal(paragraph.children.filter(({ type }) => type === "code").length, 4000);
  // A shorter run between two of one length is not taken as their closer.
  assert.deepEqual(parseInline("``a`b``"), [{ type: "code", text: "a`b" }]);
  assert.deepEqual(parseInline("`a``b` `c`"), [{ type: "code", text: "a``b" }, text(" "), { type: "code", text: "c" }]);
});

test("a reply longer than the rendered cap is cut and ends with a marker", () => {
  for (const source of [
    "word ".repeat(maxRenderedLength),
    `**start** ${"word ".repeat(maxRenderedLength / 5)}tail **end**`,
    `\`\`\`\n${"code\n".repeat(maxRenderedLength)}`,
    Array.from({ length: maxMarkdownNodes * 30 }, (_, index) => `# heading ${index}`).join("\n"),
    `${"a".repeat(maxRenderedLength - 1)}${"😀".repeat(10)}`,
  ]) {
    const blocks = parseQuickly(source);
    const shown = allText(blocks);
    assert(shown.length <= maxRenderedLength + truncationMarker.length, `${shown.length} characters shown`);
    assert.deepEqual(blocks.at(-1), { type: "paragraph", children: [text(truncationMarker)] });
    assert(!/[\uD800-\uDBFF]$/.test(shown.slice(0, -truncationMarker.length)), "a surrogate pair was split");
  }
  const long = parseQuickly(`**start** ${"word ".repeat(maxRenderedLength / 5)}`);
  assert.deepEqual(long[0]?.type === "paragraph" && long[0].children[0], { type: "strong", children: [text("start")] });
  // A reply at the cap shows in full.
  const exact = "a".repeat(maxRenderedLength);
  assert.deepEqual(parseMarkdown(exact), [{ type: "paragraph", children: [text(exact)] }]);
});
