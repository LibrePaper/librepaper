// Capture before the first await. The live session and renderer never share
// mutable text maps, file metadata or asset buffers with a copy.
/** @typedef {{main?: string, texts?: Record<string, string>, digests?: Record<string, string>, files?: Record<string, {kind?: string, sha?: string, size?: number}>, settings?: Record<string, unknown>, assets?: Record<string, Uint8Array<ArrayBufferLike>>, urls?: Record<string, string>}} PreviewTree */
/** @typedef {Required<Pick<PreviewTree, "texts"|"digests"|"files"|"assets"|"urls">> & PreviewTree} CapturedPreviewTree */
/** @param {PreviewTree} tree @returns {CapturedPreviewTree} */
export function capturePreviewTree(tree) {
  return {
    main: tree.main,
    texts: { ...tree.texts },
    digests: { ...tree.digests },
    files: Object.fromEntries(Object.entries(tree.files || {}).map(([path, entry]) => [path, { ...entry }])),
    settings: tree.settings ? { ...tree.settings } : undefined,
    assets: Object.fromEntries(Object.entries(tree.assets || {}).map(([path, bytes]) => [path, new Uint8Array(bytes).slice()])),
    urls: { ...(tree.urls || {}) },
  };
}
