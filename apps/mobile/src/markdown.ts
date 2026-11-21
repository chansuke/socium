/**
 * A small Markdown parser for assistant replies: headings, paragraphs, bold,
 * italic, inline code, fenced code, lists, quotes, rules, and links. HTML and
 * images stay plain text, and links keep only http and https targets.
 */

export type Inline =
  | { type: "text"; text: string }
  | { type: "strong"; children: Inline[] }
  | { type: "emphasis"; children: Inline[] }
  | { type: "code"; text: string }
  /** `href` is null when the target is not an http or https URL; the label shows as text. */
  | { type: "link"; href: string | null; children: Inline[] };

export interface ListItem {
  /** 0 for a top-level item. */
  depth: number;
  children: Inline[];
}

export type Block =
  | { type: "heading"; level: number; children: Inline[] }
  | { type: "paragraph"; children: Inline[] }
  | { type: "code"; language: string; text: string }
  | { type: "list"; ordered: boolean; start: number; items: ListItem[] }
  | { type: "quote"; children: Inline[] }
  | { type: "rule" };

const maxListDepth = 3;

/**
 * `raw` as a link target, or null unless it is an absolute http or https URL.
 * Checked before parsing too, so whitespace or control characters cannot
 * hide another scheme, such as `java\tscript:`.
 */
export function safeLinkUrl(raw: string): string | null {
  const value = raw.trim();
  // eslint-disable-next-line no-control-regex
  if (!/^https?:\/\/[^\s\u0000-\u001f\u007f]+$/i.test(value)) return null;
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return null;
  }
  return url.protocol === "https:" || url.protocol === "http:" ? url.href : null;
}

const fence = /^ {0,3}(`{3,}|~{3,})\s*([^`\s]*)[^`]*$/;
const heading = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$/;
const rule = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/;
const listItem = /^([ \t]*)([-*+]|\d{1,9}[.)])[ \t]+(.*)$/;
const quote = /^ {0,3}>[ \t]?(.*)$/;

const startsBlock = (line: string) =>
  fence.test(line) || heading.test(line) || rule.test(line) || listItem.test(line) || quote.test(line);

export function parseMarkdown(source: string): Block[] {
  const lines = source.replace(/\r\n?/g, "\n").split("\n");
  const blocks: Block[] = [];
  let paragraph: string[] = [];

  function endParagraph() {
    if (paragraph.length) blocks.push({ type: "paragraph", children: parseInline(joinLines(paragraph)) });
    paragraph = [];
  }

  for (let index = 0; index < lines.length; index++) {
    const line = lines[index] ?? "";
    const opened = fence.exec(line);
    if (opened) {
      endParagraph();
      const marker = opened[1] ?? "```";
      const closing = new RegExp(`^ {0,3}${marker[0] === "`" ? "`" : "~"}{${marker.length},}[ \\t]*$`);
      const body: string[] = [];
      while (++index < lines.length && !closing.test(lines[index] ?? "")) body.push(lines[index] ?? "");
      blocks.push({ type: "code", language: opened[2] ?? "", text: body.join("\n") });
      continue;
    }
    if (!line.trim()) {
      endParagraph();
      continue;
    }
    const title = heading.exec(line);
    if (title) {
      endParagraph();
      blocks.push({ type: "heading", level: title[1]?.length ?? 1, children: parseInline(title[2] ?? "") });
      continue;
    }
    if (rule.test(line)) {
      endParagraph();
      blocks.push({ type: "rule" });
      continue;
    }
    if (quote.test(line)) {
      endParagraph();
      const quoted: string[] = [];
      for (let match; index < lines.length && (match = quote.exec(lines[index] ?? "")); index++) quoted.push(match[1] ?? "");
      index--;
      blocks.push({ type: "quote", children: parseInline(joinLines(quoted)) });
      continue;
    }
    const item = listItem.exec(line);
    if (item) {
      endParagraph();
      index = parseList(lines, index, blocks);
      continue;
    }
    paragraph.push(line);
  }
  endParagraph();
  return blocks;
}

/** Reads the list that starts at `start`; returns the index of its last line. */
function parseList(lines: string[], start: number, blocks: Block[]) {
  const first = listItem.exec(lines[start] ?? "");
  const ordered = /\d/.test(first?.[2] ?? "");
  const list: Block & { type: "list" } = { type: "list", ordered, start: ordered ? Number.parseInt(first?.[2] ?? "1", 10) : 1, items: [] };
  blocks.push(list);
  const baseIndent = indentOf(first?.[1] ?? "");
  let text: string[] = [];
  let depth = 0;
  const endItem = () => {
    if (text.length) list.items.push({ depth, children: parseInline(joinLines(text)) });
    text = [];
  };

  let index = start;
  for (; index < lines.length; index++) {
    const line = lines[index] ?? "";
    const item = listItem.exec(line);
    if (item) {
      const nested = indentOf(item[1] ?? "") > baseIndent;
      // A top-level marker of the other kind starts a new list.
      if (!nested && /\d/.test(item[2] ?? "") !== ordered) break;
      endItem();
      depth = nested ? Math.min(maxListDepth, Math.max(1, Math.floor((indentOf(item[1] ?? "") - baseIndent) / 2))) : 0;
      text = [item[3] ?? ""];
      continue;
    }
    if (!line.trim()) {
      // A blank line ends the list unless another item follows it.
      const next = lines.slice(index + 1).find((candidate) => candidate.trim());
      if (next !== undefined && listItem.test(next)) continue;
      break;
    }
    if (startsBlock(line)) break;
    text.push(line.trim());
  }
  endItem();
  return index - 1;
}

function indentOf(whitespace: string) {
  return whitespace.replace(/\t/g, "    ").length;
}

/** Joins a paragraph's lines; two trailing spaces or a backslash keep the break. */
function joinLines(lines: string[]) {
  return lines.map((line, index) => {
    if (index === lines.length - 1) return line.trim();
    if (/ {2,}$/.test(line)) return `${line.trim()}\n`;
    if (/\\$/.test(line)) return `${line.trim().slice(0, -1)}\n`;
    return `${line.trim()} `;
  }).join("");
}

const punctuation = /[!-/:-@[-`{-~]/;
const isWordCharacter = (character: string | undefined) => character !== undefined && /[\p{L}\p{N}]/u.test(character);
const isSpace = (character: string | undefined) => character === undefined || /\s/.test(character);

export function parseInline(text: string): Inline[] {
  const result: Inline[] = [];
  let plain = "";
  const pushText = () => {
    if (plain) result.push({ type: "text", text: plain });
    plain = "";
  };
  const push = (inline: Inline) => {
    pushText();
    result.push(inline);
  };

  let index = 0;
  while (index < text.length) {
    const character = text[index] ?? "";
    const rest = text.slice(index);

    if (character === "\\" && punctuation.test(text[index + 1] ?? "")) {
      plain += text[index + 1];
      index += 2;
      continue;
    }

    if (character === "`") {
      const run = /^`+/.exec(rest)?.[0] ?? "`";
      const close = findRun(text, "`", run.length, index + run.length);
      if (close >= 0) {
        let code = text.slice(index + run.length, close).replace(/\n/g, " ");
        if (/^ .*[^ ].* $/.test(code)) code = code.slice(1, -1);
        push({ type: "code", text: code });
        index = close + run.length;
      } else {
        plain += run;
        index += run.length;
      }
      continue;
    }

    if (character === "[") {
      const link = readLink(text, index);
      if (link) {
        push({ type: "link", href: safeLinkUrl(link.target), children: parseInline(link.label) });
        index = link.end;
        continue;
      }
    }

    if (character === "<") {
      const autolink = /^<(https?:\/\/[^\s<>]+)>/i.exec(rest);
      if (autolink?.[1]) {
        push({ type: "link", href: safeLinkUrl(autolink[1]), children: [{ type: "text", text: autolink[1] }] });
        index += autolink[0].length;
        continue;
      }
    }

    if ((character === "h" || character === "H") && !isWordCharacter(text[index - 1])) {
      const bare = /^https?:\/\/[^\s<>]+/i.exec(rest)?.[0];
      if (bare) {
        const url = trimTrailingPunctuation(bare);
        push({ type: "link", href: safeLinkUrl(url), children: [{ type: "text", text: url }] });
        index += url.length;
        continue;
      }
    }

    if (character === "*" || character === "_") {
      const emphasis = readEmphasis(text, index, character);
      if (emphasis) {
        push(emphasis.inline);
        index = emphasis.end;
        continue;
      }
      const run = character === "*" ? /^\*+/.exec(rest)?.[0] : /^_+/.exec(rest)?.[0];
      plain += run ?? character;
      index += run?.length ?? 1;
      continue;
    }

    plain += character;
    index++;
  }
  pushText();
  return result;
}

/** The index of the next run of exactly `length` copies of `character` from `from`, or -1. */
function findRun(text: string, character: string, length: number, from: number) {
  for (let index = text.indexOf(character, from); index >= 0; index = text.indexOf(character, index + 1)) {
    let end = index;
    while (text[end] === character) end++;
    if (end - index === length) return index;
    index = end - 1;
  }
  return -1;
}

function readEmphasis(text: string, start: number, character: "*" | "_") {
  let length = 0;
  while (text[start + length] === character) length++;
  if (length > 3) return null;
  const opensBefore = text[start - 1];
  const after = text[start + length];
  // An opening run is followed by text; `_` inside a word, as in snake_case, is literal.
  if (isSpace(after) || (character === "_" && isWordCharacter(opensBefore))) return null;
  for (let close = findRun(text, character, length, start + length); close >= 0; close = findRun(text, character, length, close + length)) {
    const before = text[close - 1];
    const following = text[close + length];
    if (isSpace(before) || (character === "_" && isWordCharacter(following))) continue;
    const children = parseInline(text.slice(start + length, close));
    const inline: Inline = length === 1 ? { type: "emphasis", children }
      : length === 2 ? { type: "strong", children }
        : { type: "strong", children: [{ type: "emphasis", children }] };
    return { inline, end: close + length };
  }
  return null;
}

/** Reads `[label](target "title")` at `start`. */
function readLink(text: string, start: number) {
  let depth = 0;
  let labelEnd = -1;
  for (let index = start; index < text.length; index++) {
    const character = text[index];
    if (character === "\\") {
      index++;
    } else if (character === "[") {
      depth++;
    } else if (character === "]" && --depth === 0) {
      labelEnd = index;
      break;
    }
  }
  if (labelEnd < 0 || text[labelEnd + 1] !== "(") return null;
  let parens = 0;
  for (let index = labelEnd + 2; index < text.length; index++) {
    const character = text[index];
    if (character === "(") parens++;
    else if (character === ")" && parens-- === 0) {
      const inside = text.slice(labelEnd + 2, index).trim();
      const target = /^<([^>]*)>/.exec(inside)?.[1] ?? inside.split(/\s+/)[0] ?? "";
      return { label: text.slice(start + 1, labelEnd), target, end: index + 1 };
    }
  }
  return null;
}

/** Drops sentence punctuation after a bare URL, keeping a `)` that closes a `(` in it. */
function trimTrailingPunctuation(url: string) {
  let end = url.length;
  for (;;) {
    const last = url[end - 1];
    if (last && /[.,:;!?'"*_]/.test(last)) {
      end--;
    } else if (last === ")" && (url.slice(0, end).match(/\(/g)?.length ?? 0) < (url.slice(0, end).match(/\)/g)?.length ?? 0)) {
      end--;
    } else {
      return url.slice(0, end);
    }
  }
}
