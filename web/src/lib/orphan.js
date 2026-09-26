// The source attachment answers whether the passage survived. Rendered words
// only say where to paint it. Readers do not receive source provenance, so
// their cards can only describe whether the published page still shows it.
export function orphanState({ renderedFound, sourceFound, sourceKnown = false, wholeDocument = false }) {
  if (wholeDocument) return { orphaned: false, inSourceOnly: false };
  return {
    orphaned: sourceKnown ? !sourceFound : !renderedFound,
    inSourceOnly: sourceKnown && !renderedFound && Boolean(sourceFound),
  };
}
