// The directory watcher is called once per commit with that commit's events,
// and it reaches inside the files map, so it hears an edit to any text. Every
// such edit repaints the preview, because the preview may be locked on a file
// other than the one that changed. A batch with no events is the only one that
// has nothing to repaint.
export function needsSourceRefresh(events) {
  return !Array.isArray(events) || events.length > 0;
}
