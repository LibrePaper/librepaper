// A component loaded on first use, so the code of a panel or dialog that is
// not on screen at open stays out of the first load. `component` is null until
// the module has arrived; `load()` starts the import once and can be awaited.
export function lazyComponent(loader) {
  let component = $state.raw(null);
  let pending = null;
  return {
    get component() {
      return component;
    },
    load() {
      pending ??= loader().then(
        (module) => (component = module.default),
        (error) => {
          pending = null;
          throw error;
        },
      );
      return pending;
    },
  };
}
