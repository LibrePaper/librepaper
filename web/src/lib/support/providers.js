// Where the parts of the app say what state they are in, for the report.
//
// A component registers a function under a name when it mounts and removes it
// when it goes. The report asks for every provider at the moment it is built.
// Nothing is pushed, so nothing is kept after the page is gone.

const providers = new Map();

/// Registers `fn` under `name`. A later call with the same name replaces the
/// earlier one. Returns the way to unregister it.
export function provide(name, fn) {
  providers.set(name, fn);
  return () => {
    if (providers.get(name) === fn) providers.delete(name);
  };
}

/// `{ [name]: value }` for every provider. A provider that throws yields
/// `{ error: "unavailable" }` and does not stop the others.
export function snapshot() {
  const result = {};
  for (const [name, fn] of providers) {
    try {
      result[name] = fn();
    } catch {
      result[name] = { error: "unavailable" };
    }
  }
  return result;
}
