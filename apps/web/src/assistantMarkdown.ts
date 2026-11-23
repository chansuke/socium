import { createElement, Fragment } from "react";
import type { ReactNode } from "react";
import Markdown from "react-markdown";
import { safeHttpUrl } from "@socium/client/links";

/** A link with an allowed target opens in a new tab; one without shows as its label. */
function SafeLink({ href, children }: { href?: string; children?: ReactNode }) {
  if (!href) return createElement(Fragment, null, children);
  return createElement("a", { href, target: "_blank", rel: "noreferrer noopener" }, children);
}

/**
 * An assistant reply as Markdown. HTML and images are dropped, and links keep
 * only absolute http and https targets: `mailto:`, `javascript:`, relative
 * URLs, and every other target show as the link's label.
 */
export function AssistantMarkdown({ text }: { text: string }) {
  return createElement(Markdown, {
    skipHtml: true,
    disallowedElements: ["img"],
    urlTransform: (url) => safeHttpUrl(url),
    components: { a: SafeLink },
  }, text);
}
