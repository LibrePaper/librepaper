/*
 * Filtering for the Outline panel. A heading that matches a filter is shown.
 * The headings above it stay visible as context, marked so the panel can grey
 * them, so the tree still reads. Headings carry Org fields (`todo`, `done`,
 * `tags`) only in Org; other formats match no TODO or tag filter.
 */

export const OUTLINE_FILTER_ALL = "all";
export const OUTLINE_FILTER_OPEN = "open";
const TAG_PREFIX = "tag:";

/** The filter value that shows the headings carrying one tag. */
export function tagFilter(tag) {
  return `${TAG_PREFIX}${tag}`;
}

/** The distinct tags of the headings, sorted. */
export function outlineTags(headings) {
  const tags = new Set();
  for (const heading of headings) for (const tag of heading.tags ?? []) tags.add(tag);
  return [...tags].sort((a, b) => a.localeCompare(b));
}

/** Whether a filter is worth offering: some heading has a TODO state or a tag. */
export function hasOutlineFilter(headings) {
  return headings.some((heading) => heading.todo || heading.tags?.length);
}

function matches(heading, filter) {
  if (filter === OUTLINE_FILTER_ALL) return true;
  if (filter === OUTLINE_FILTER_OPEN) return Boolean(heading.todo) && !heading.done;
  if (filter.startsWith(TAG_PREFIX)) return heading.tags?.includes(filter.slice(TAG_PREFIX.length)) ?? false;
  return false;
}

/**
 * The headings to show under a filter, in source order, as
 * `{ heading, index, context }`. `index` is the heading's position in
 * `headings`, so per-heading data stays aligned. `context` is true for a
 * heading shown only because a heading below it (within its section) matches.
 */
export function filterOutline(headings, filter = OUTLINE_FILTER_ALL) {
  const rows = [];
  headings.forEach((heading, index) => {
    if (matches(heading, filter)) {
      rows.push({ heading, index, context: false });
      return;
    }
    // Headings below this one with a greater level are its descendants; the
    // first heading at the same or a higher level ends them.
    for (let next = index + 1; next < headings.length && headings[next].level > heading.level; next++) {
      if (matches(headings[next], filter)) {
        rows.push({ heading, index, context: true });
        return;
      }
    }
  });
  return rows;
}
