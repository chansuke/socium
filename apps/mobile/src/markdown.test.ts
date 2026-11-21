import { strict as assert } from "node:assert";
import { test } from "node:test";
import { parseInline, parseMarkdown, safeLinkUrl } from "./markdown.ts";
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
      { depth: 0, children: [text("third continued")] },
      { depth: 0, children: [text("fourth")] },
      { depth: 1, children: [text("nested")] },
    ] },
    { type: "list", ordered: false, start: 1, items: [
      { depth: 0, children: [text("other list")] },
      { depth: 0, children: [text("same list")] },
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
