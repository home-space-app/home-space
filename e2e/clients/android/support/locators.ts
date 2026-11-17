// XPath rather than UiSelector: right after the app restarts, a UiSelector lookup sometimes
// misses Compose nodes that the hierarchy dump (which XPath searches) already has.

/** The element with Compose test tag `tag`, which the app exposes as its resource id. */
export function tag(tag: string): string {
  return `//*[@resource-id=${xpathString(tag)}]`;
}

/** The element whose text is `text`. */
export function text(text: string): string {
  return `//*[@text=${xpathString(text)}]`;
}

/** The element whose content description (its accessible name) is `label`. */
export function label(label: string): string {
  return `//*[@content-desc=${xpathString(label)}]`;
}

/** A string literal for XPath 1.0, which has no escapes. */
export function xpathString(value: string): string {
  if (!value.includes('"')) return `"${value}"`;
  if (!value.includes("'")) return `'${value}'`;
  return `concat(${value
    .split('"')
    .map((part) => `"${part}"`)
    .join(`, '"', `)})`;
}
