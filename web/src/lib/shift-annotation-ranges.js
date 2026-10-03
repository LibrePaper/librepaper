/// Carry annotation offsets through one source edit. A common prefix/suffix
/// isolates the replaced interval; offsets after it move with the length
/// delta, while offsets inside it clamp to the corresponding new edge.
export function shiftAnnotationRanges(before, after, ranges) {
  let start = 0;
  while (start < before.length && start < after.length && before[start] === after[start]) start++;
  let suffix = 0;
  while (suffix < before.length - start && suffix < after.length - start &&
      before[before.length - suffix - 1] === after[after.length - suffix - 1]) suffix++;
  const oldEnd = before.length - suffix;
  const newEnd = after.length - suffix;
  const delta = newEnd - oldEnd;
  if (start === oldEnd && delta === 0) return ranges;

  const moveStart = (offset) => offset < start ? offset : offset >= oldEnd ? offset + delta : start;
  // An insertion exactly at an annotation's end remains outside it; if the
  // range crosses the insertion, its end moves and the inserted text is kept.
  const moveEnd = (offset) => offset <= start ? offset : offset >= oldEnd ? offset + delta : newEnd;
  return ranges.map((range) => {
    const nextStart = moveStart(range.start);
    const nextEnd = range.start === range.end ? nextStart : moveEnd(range.end);
    return { ...range, start: Math.max(0, Math.min(after.length, nextStart)), end: Math.max(0, Math.min(after.length, nextEnd)) };
  });
}
