import { strict as assert } from "node:assert";
import { test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { AssistantMarkdown } from "./assistantMarkdown.ts";

const render = (text: string) => renderToStaticMarkup(createElement(AssistantMarkdown, { text }));

test("http and https links open in a new tab", () => {
  assert.equal(
    render("[docs](https://example.com/docs) and <http://example.com/>"),
    "<p><a href=\"https://example.com/docs\" target=\"_blank\" rel=\"noreferrer noopener\">docs</a> and "
      + "<a href=\"http://example.com/\" target=\"_blank\" rel=\"noreferrer noopener\">http://example.com/</a></p>",
  );
});

test("links to any other target show as plain text", () => {
  for (const [markdown, label] of [
    ["[mail](mailto:someone@example.com)", "mail"],
    ["<mailto:someone@example.com>", "mailto:someone@example.com"],
    ["[chat](irc://irc.example.com/socium)", "chat"],
    ["[chat](ircs://irc.example.com/socium)", "chat"],
    ["[chat](xmpp:someone@example.com)", "chat"],
    ["[run](javascript:alert(1))", "run"],
    ["[data](data:text/html,hi)", "data"],
    ["[here](/relative)", "here"],
    ["[ref][r]\n\n[r]: ./relative", "ref"],
  ]) {
    const html = render(markdown);
    assert.equal(html, `<p>${label}</p>`, markdown);
  }
});

test("HTML and images are not rendered", () => {
  assert.equal(render("<b>bold</b> ![alt](https://example.com/a.png)"), "<p>bold </p>");
});
