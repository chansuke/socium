/**
 * `raw` as a link target if it is an absolute http or https URL, otherwise
 * null, so the link shows as its label alone. Whitespace and control
 * characters are refused before parsing, so they cannot hide another scheme,
 * such as `java\tscript:`.
 */
export function safeHttpUrl(raw: string): string | null {
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
