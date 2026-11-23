/**
 * A small Markdown parser for assistant replies: headings, paragraphs, bold,
 * italic, inline code, fenced code, lists, quotes, rules, and links. HTML and
 * images stay plain text, and links keep only http and https targets.
 *
 * A reply comes from the model, so the parser must stay fast on any input:
 * each line is read once, delimiters are matched from tables built in one
 * pass, no regular expression can backtrack over a long run of one
 * character, and nesting, length, and block count are capped.
 */
import { safeHttpUrl } from "@socium/client/links";

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
  /** The number a top-level item of an ordered list shows; null otherwise. */
  number: number | null;
  children: Inline[];
}

export type Block =
  | { type: "heading"; level: number; children: Inline[] }
  | { type: "paragraph"; children: Inline[] }
  | { type: "code"; language: string; text: string }
  | { type: "list"; ordered: boolean; start: number; items: ListItem[] }
  | { type: "quote"; children: Inline[] }
  | { type: "rule" };

/** Characters read as Markdown; the rest of a longer reply shows as plain text. */
export const maxMarkdownLength = 50_000;
/** Blocks and list items read as Markdown; the lines after them show as plain text. */
export const maxMarkdownNodes = 1000;
/** How deep emphasis and links nest; text deeper than this shows as written. */
export const maxInlineDepth = 4;

const maxListDepth = 3;

const isBlank = (line: string) => line.trim() === "";
const isSpaceOrTab = (character: string | undefined) => character === " " || character === "\t";

/** The index after up to three leading spaces. */
function afterIndent(line: string) {
  let at = 0;
  while (at < 3 && line[at] === " ") at++;
  return at;
}

/** The index after the run of `character` that starts at `from`. */
function runEnd(text: string, character: string, from: number) {
  let end = from;
  while (text[end] === character) end++;
  return end;
}

function readFence(line: string) {
  const at = afterIndent(line);
  const marker = line[at];
  if (marker !== "`" && marker !== "~") return null;
  const end = runEnd(line, marker, at);
  const info = line.slice(end);
  if (end - at < 3 || info.includes("`")) return null;
  const words = info.trim();
  let wordEnd = 0;
  while (wordEnd < words.length && !/\s/.test(words[wordEnd] ?? "")) wordEnd++;
  return { marker, length: end - at, language: words.slice(0, wordEnd) };
}

function closesFence(line: string, fence: { marker: string; length: number }) {
  const at = afterIndent(line);
  const end = runEnd(line, fence.marker, at);
  return end - at >= fence.length && !/[^ \t]/.test(line.slice(end));
}

function readHeading(line: string) {
  const at = afterIndent(line);
  const end = runEnd(line, "#", at);
  const level = end - at;
  if (level < 1 || level > 6 || !(end === line.length || isSpaceOrTab(line[end]))) return null;
  let title = line.slice(end).trim();
  // A closing run of `#` goes, if a space or tab comes before it.
  let close = title.length;
  while (close > 0 && title[close - 1] === "#") close--;
  if (close === 0) title = "";
  else if (close < title.length && isSpaceOrTab(title[close - 1])) title = title.slice(0, close).trimEnd();
  return { level, title };
}

function isRule(line: string) {
  const at = afterIndent(line);
  const marker = line[at];
  if (marker !== "-" && marker !== "*" && marker !== "_") return false;
  let count = 0;
  for (let index = at; index < line.length; index++) {
    if (line[index] === marker) count++;
    else if (!isSpaceOrTab(line[index])) return false;
  }
  return count >= 3;
}

function readQuote(line: string) {
  const at = afterIndent(line);
  if (line[at] !== ">") return null;
  return line.slice(isSpaceOrTab(line[at + 1]) ? at + 2 : at + 1);
}

function readListItem(line: string) {
  let at = 0;
  while (isSpaceOrTab(line[at])) at++;
  const indent = line.slice(0, at);
  let markerEnd = at;
  let ordered = false;
  if (line[at] === "-" || line[at] === "*" || line[at] === "+") {
    markerEnd = at + 1;
  } else {
    while (markerEnd - at < 9 && /[0-9]/.test(line[markerEnd] ?? "")) markerEnd++;
    if (markerEnd === at || (line[markerEnd] !== "." && line[markerEnd] !== ")")) return null;
    ordered = true;
    markerEnd++;
  }
  if (!isSpaceOrTab(line[markerEnd])) return null;
  let textStart = markerEnd;
  while (isSpaceOrTab(line[textStart])) textStart++;
  return { indent, ordered, number: ordered ? Number.parseInt(line.slice(at, markerEnd - 1), 10) : null, text: line.slice(textStart) };
}

const startsBlock = (line: string) =>
  readFence(line) !== null || readHeading(line) !== null || isRule(line) || readListItem(line) !== null || readQuote(line) !== null;

const plainBlock = (text: string): Block => ({ type: "paragraph", children: text ? [{ type: "text", text }] : [] });

export function parseMarkdown(source: string): Block[] {
  const read = source.slice(0, maxMarkdownLength);
  const lines = read.replace(/\r\n?/g, "\n").split("\n");
  /** The index of the first line after each line that is not blank, or `lines.length`. */
  const nextFilled = new Int32Array(lines.length + 1).fill(lines.length);
  for (let index = lines.length - 2; index >= 0; index--) {
    nextFilled[index] = isBlank(lines[index + 1] ?? "") ? nextFilled[index + 1] ?? lines.length : index + 1;
  }
  const blocks: Block[] = [];
  let nodes = 0;
  let paragraph: string[] = [];

  function push(block: Block) {
    blocks.push(block);
    nodes++;
  }
  function endParagraph() {
    if (paragraph.length) push({ type: "paragraph", children: parseInline(joinLines(paragraph)) });
    paragraph = [];
  }

  let index = 0;
  for (; index < lines.length && nodes < maxMarkdownNodes; index++) {
    const line = lines[index] ?? "";
    const fence = readFence(line);
    if (fence) {
      endParagraph();
      const body: string[] = [];
      while (++index < lines.length && !closesFence(lines[index] ?? "", fence)) body.push(lines[index] ?? "");
      push({ type: "code", language: fence.language, text: body.join("\n") });
      continue;
    }
    if (isBlank(line)) {
      endParagraph();
      continue;
    }
    const heading = readHeading(line);
    if (heading) {
      endParagraph();
      push({ type: "heading", level: heading.level, children: parseInline(heading.title) });
      continue;
    }
    if (isRule(line)) {
      endParagraph();
      push({ type: "rule" });
      continue;
    }
    if (readQuote(line) !== null) {
      endParagraph();
      const quoted: string[] = [];
      for (let text; index < lines.length && (text = readQuote(lines[index] ?? "")) !== null; index++) quoted.push(text);
      index--;
      push({ type: "quote", children: parseInline(joinLines(quoted)) });
      continue;
    }
    if (readListItem(line)) {
      endParagraph();
      // No room for the list and one item.
      if (nodes + 2 > maxMarkdownNodes) break;
      const list = parseList(lines, index, nextFilled, maxMarkdownNodes - nodes - 1);
      push(list.block);
      nodes += list.block.items.length;
      index = list.last;
      continue;
    }
    paragraph.push(line);
  }
  endParagraph();
  // Past the limits, the reply still shows in full, as written.
  const rest = [lines.slice(index).join("\n"), source.slice(read.length)].filter(Boolean).join("");
  if (rest.trim()) blocks.push(plainBlock(rest));
  return blocks;
}

/** Reads at most `budget` items of the list that starts at `start`; `last` is the index of its last line. */
function parseList(lines: string[], start: number, nextFilled: Int32Array, budget: number) {
  const first = readListItem(lines[start] ?? "");
  const ordered = first?.ordered ?? false;
  const block: Block & { type: "list" } = { type: "list", ordered, start: first?.number ?? 1, items: [] };
  const baseIndent = indentOf(first?.indent ?? "");
  let number = block.start - 1;
  let text: string[] = [];
  let depth = 0;
  const endItem = () => {
    if (text.length) {
      if (depth === 0) number++;
      block.items.push({ depth, number: ordered && depth === 0 ? number : null, children: parseInline(joinLines(text)) });
    }
    text = [];
  };

  let index = start;
  for (; index < lines.length; index++) {
    const line = lines[index] ?? "";
    const item = readListItem(line);
    if (item) {
      const nested = indentOf(item.indent) > baseIndent;
      // A top-level marker of the other kind starts a new list.
      if (!nested && item.ordered !== ordered) break;
      endItem();
      if (block.items.length >= budget) break;
      depth = nested ? Math.min(maxListDepth, Math.max(1, Math.floor((indentOf(item.indent) - baseIndent) / 2))) : 0;
      text = [item.text];
      continue;
    }
    if (isBlank(line)) {
      // A blank line ends the list unless another item follows it.
      const next = nextFilled[index] ?? lines.length;
      if (next < lines.length && readListItem(lines[next] ?? "")) continue;
      break;
    }
    if (startsBlock(line)) break;
    text.push(line.trim());
  }
  if (block.items.length < budget) endItem();
  return { block, last: index - 1 };
}

function indentOf(whitespace: string) {
  return whitespace.replace(/\t/g, "    ").length;
}

/** Joins a paragraph's lines; two trailing spaces or a backslash keep the break. */
function joinLines(lines: string[]) {
  return lines.map((line, index) => {
    if (index === lines.length - 1) return line.trim();
    if (line.endsWith("  ")) return `${line.trim()}\n`;
    if (line.endsWith("\\")) return `${line.trim().slice(0, -1)}\n`;
    return `${line.trim()} `;
  }).join("");
}

const punctuation = /[!-/:-@[-`{-~]/;
const isWordCharacter = (character: string | undefined) => character !== undefined && /[\p{L}\p{N}]/u.test(character);
const isSpace = (character: string | undefined) => character === undefined || /\s/.test(character);
/** Sticky, so each match starts where the parser is instead of searching ahead. */
const autolinkPattern = /<(https?:\/\/[^\s<>]+)>/iy;
const bareUrlPattern = /https?:\/\/[^\s<>]+/iy;

/** The first value in sorted `values` at or after `from`, or -1. */
function firstFrom(values: number[] | undefined, from: number) {
  if (!values) return -1;
  let low = 0;
  let high = values.length;
  while (low < high) {
    const middle = (low + high) >> 1;
    if ((values[middle] ?? 0) < from) low = middle + 1;
    else high = middle;
  }
  return values[low] ?? -1;
}

/**
 * Where each delimiter of `text` closes, found in one pass: the `]` for each
 * `[` and the `)` for each `(`, and the start of every run of backticks and
 * of every run of `*` or `_` that can close emphasis, by its length.
 */
function scanDelimiters(text: string) {
  const bracket = new Int32Array(text.length).fill(-1);
  const paren = new Int32Array(text.length).fill(-1);
  const backticks = new Map<number, number[]>();
  const closers = new Map<string, number[]>();
  const brackets: number[] = [];
  const parens: number[] = [];
  for (let index = 0; index < text.length; index++) {
    const character = text[index];
    if (character === "\\") {
      // Escapes count inside link labels only, as before: parens ignore them.
      if (text[index + 1] === "(") parens.push(index + 1);
      else if (text[index + 1] === ")") {
        const open = parens.pop();
        if (open !== undefined) paren[open] = index + 1;
      }
      index++;
    } else if (character === "[") {
      brackets.push(index);
    } else if (character === "]") {
      const open = brackets.pop();
      if (open !== undefined) bracket[open] = index;
    } else if (character === "(") {
      parens.push(index);
    } else if (character === ")") {
      const open = parens.pop();
      if (open !== undefined) paren[open] = index;
    }
  }
  for (let index = 0; index < text.length;) {
    const character = text[index] ?? "";
    if (character !== "`" && character !== "*" && character !== "_") {
      index++;
      continue;
    }
    const end = runEnd(text, character, index);
    const length = end - index;
    if (character === "`") {
      const starts = backticks.get(length) ?? [];
      starts.push(index);
      backticks.set(length, starts);
    } else if (length <= 3 && !isSpace(text[index - 1]) && !(character === "_" && isWordCharacter(text[end]))) {
      const key = `${character}${length}`;
      const starts = closers.get(key) ?? [];
      starts.push(index);
      closers.set(key, starts);
    }
    index = end;
  }
  return { bracket, paren, backticks, closers };
}

export function parseInline(text: string, depth = 0): Inline[] {
  if (depth >= maxInlineDepth) return text ? [{ type: "text", text }] : [];
  const { bracket, paren, backticks, closers } = scanDelimiters(text);
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

    if (character === "\\" && punctuation.test(text[index + 1] ?? "")) {
      plain += text[index + 1];
      index += 2;
      continue;
    }

    if (character === "`") {
      const length = runEnd(text, "`", index) - index;
      const close = firstFrom(backticks.get(length), index + length);
      if (close >= 0) {
        let code = text.slice(index + length, close).replace(/\n/g, " ");
        if (code.length > 2 && code.startsWith(" ") && code.endsWith(" ") && code.trim() !== "") code = code.slice(1, -1);
        push({ type: "code", text: code });
        index = close + length;
      } else {
        plain += text.slice(index, index + length);
        index += length;
      }
      continue;
    }

    if (character === "[") {
      const labelEnd = bracket[index] ?? -1;
      const targetEnd = labelEnd >= 0 && text[labelEnd + 1] === "(" ? paren[labelEnd + 1] ?? -1 : -1;
      if (targetEnd >= 0) {
        const inside = text.slice(labelEnd + 2, targetEnd).trim();
        const target = /^<([^>]*)>/.exec(inside)?.[1] ?? inside.split(/\s/)[0] ?? "";
        push({ type: "link", href: safeHttpUrl(target), children: parseInline(text.slice(index + 1, labelEnd), depth + 1) });
        index = targetEnd + 1;
        continue;
      }
    }

    if (character === "<") {
      autolinkPattern.lastIndex = index;
      const autolink = autolinkPattern.exec(text);
      if (autolink?.[1]) {
        push({ type: "link", href: safeHttpUrl(autolink[1]), children: [{ type: "text", text: autolink[1] }] });
        index += autolink[0].length;
        continue;
      }
    }

    if ((character === "h" || character === "H") && !isWordCharacter(text[index - 1])) {
      bareUrlPattern.lastIndex = index;
      const bare = bareUrlPattern.exec(text)?.[0];
      if (bare) {
        const url = trimTrailingPunctuation(bare);
        push({ type: "link", href: safeHttpUrl(url), children: [{ type: "text", text: url }] });
        index += url.length;
        continue;
      }
    }

    if (character === "*" || character === "_") {
      const end = runEnd(text, character, index);
      const length = end - index;
      // An opening run is followed by text; `_` inside a word, as in snake_case, is literal.
      const opens = length <= 3 && !isSpace(text[end]) && !(character === "_" && isWordCharacter(text[index - 1]));
      const close = opens ? firstFrom(closers.get(`${character}${length}`), end) : -1;
      if (close >= 0) {
        const children = parseInline(text.slice(end, close), depth + 1);
        push(length === 1 ? { type: "emphasis", children }
          : length === 2 ? { type: "strong", children }
            : { type: "strong", children: [{ type: "emphasis", children }] });
        index = close + length;
      } else {
        plain += text.slice(index, end);
        index = end;
      }
      continue;
    }

    plain += character;
    index++;
  }
  pushText();
  return result;
}

/** Drops sentence punctuation after a bare URL, keeping a `)` that closes a `(` in it. */
function trimTrailingPunctuation(url: string) {
  let opens = 0;
  let closes = 0;
  for (const character of url) {
    if (character === "(") opens++;
    else if (character === ")") closes++;
  }
  let end = url.length;
  for (;;) {
    const last = url[end - 1];
    if (last && /[.,:;!?'"*_]/.test(last)) {
      end--;
    } else if (last === ")" && opens < closes) {
      end--;
      closes--;
    } else {
      return url.slice(0, end);
    }
  }
}
