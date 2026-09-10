// Where the dictation service says things to the reader.
//
// The toast store lives in toast.svelte.js, which pulls Skeleton's Svelte
// components in with it. The service cannot import that: its checks run it
// under Node, and a dynamic import() instead would become a second chunk in
// every single-file harness build that mounts the reader, which their small
// static servers do not serve. So the service talks to this slot, and the
// dictation components fill it when they mount. Until one has, a message is
// dropped, which is right for a page with no dictation on it.
let notifier = () => {};

export function setNotifier(fn) {
  notifier = fn || (() => {});
}

export function notify(message, level) {
  notifier(message, level);
}
