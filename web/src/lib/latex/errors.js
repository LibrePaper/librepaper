// Named errors, built in one line instead of three.
//
// The compile path signals its cases by `error.name` -- `Unreachable`,
// `Unauthorized`, `Refused`, `Canceled`, `Superseded`, `NotRendered`,
// `VmUnsupported`, `VmUnavailable`, `WorkerTimeout` -- because callers branch
// on them and a subclass per case would be nine classes for no gain. Fifteen
// sites each wrote the same three statements to do it; this is that, once,
// and it makes the set of names greppable from one place.
//
// `latex/vm-worker.js` is a classic worker and cannot import this, so its one
// `Canceled` stays written out by hand.

/// An `Error` carrying `name`, plus any extra fields the caller's branch
/// reads off it (`status` on a refusal, `rendering` on a missing render).
export function named(name, message, fields) {
  const error = new Error(message);
  error.name = name;
  if (fields) Object.assign(error, fields);
  return error;
}
