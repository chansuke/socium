import { strict as assert } from "node:assert";
import { test } from "node:test";
import { safeHttpUrl } from "./links.ts";

test("only absolute http and https URLs are link targets", () => {
  assert.equal(safeHttpUrl("https://example.com/a?b=c#d"), "https://example.com/a?b=c#d");
  assert.equal(safeHttpUrl("HTTP://Example.com"), "http://example.com/");
  assert.equal(safeHttpUrl(" https://example.com "), "https://example.com/");
  for (const rejected of [
    "mailto:someone@example.com", "irc://irc.example.com/socium", "ircs://irc.example.com/socium", "xmpp:someone@example.com",
    "javascript:alert(1)", "JaVaScRiPt:alert(1)", "java\tscript:alert(1)", "data:text/html,<script>alert(1)</script>",
    "vbscript:msgbox(1)", "file:///etc/passwd", "/relative", "relative/path", "./here", "#anchor", "//example.com/protocol-relative",
    "https://exa mple.com", "https://example.com/\u0000", "https:example.com", "",
  ]) {
    assert.equal(safeHttpUrl(rejected), null, rejected);
  }
});
