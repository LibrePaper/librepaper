// Whether a comment's passage counts as lost, now that it can be looked for
// in two places: the rendered page, which is what the highlight and the
// click target are drawn from, and the source, the anchor of record. Kept as
// its own pure rule -- no comment shape, no tree, nothing to mock -- because
// the two anchors are computed in different modules and this is the one
// place their answers are combined.
//
// A comment is orphaned only when neither anchor finds its passage. When the
// source still has it but the rendering does not, the passage is not lost --
// it is only unreachable from the page, which the card says instead of
// "orphaned" and which a reveal answers by going to the source. A region
// annotation is placed by the agent, not by either quotation, so it is never
// orphaned and never source-only.
export function orphanState({ renderedFound, sourceFound, region }) {
  if (region) return { orphaned: false, inSourceOnly: false };
  return {
    orphaned: !renderedFound && !sourceFound,
    inSourceOnly: !renderedFound && Boolean(sourceFound),
  };
}
