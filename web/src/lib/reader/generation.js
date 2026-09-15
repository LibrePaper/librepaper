// "Is this run still the current one?", written once.
//
// Eight counters across seven files -- boot, collaboration, history-source,
// timeline, passage-trace, local-preview twice, and Reader itself -- had
// hand-rolled the same thing: bump an integer when a run starts, capture it in
// a local, and compare it back after every await so a slow answer cannot
// overwrite a newer one. The counter was always right, but each copy re-stated
// the invariant in its own words, and a comparison written the wrong way round
// fails silently and late -- the class of bug that shows up only as a stale
// render nobody can reproduce.
//
// `begin()` starts a run and hands back the only thing a caller needs: a
// predicate that answers whether it has since been overtaken. The counter
// itself stays in here, where no caller can compare it incorrectly.
export function createGeneration() {
  let current = 0;
  const at = (mine) => () => mine !== current;

  return {
    /// Start a run. Every run begun earlier is now stale.
    begin: () => at(++current),
    /// Follow the run already in progress without starting one, for work that
    /// belongs to the current run rather than replacing it.
    mark: () => at(current),
    /// Abandon whatever is in flight: teardown, or an explicit invalidation.
    cancel() { current += 1; },
  };
}
