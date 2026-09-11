// Content-authorship evidence and bounded source-only refinement.
//
// Checkpoint `by` is an event actor. It is intentionally not consulted here:
// a publisher, sync sender, or automatic-checkpoint writer is not evidence
// that they authored the bytes in the interval.

export const MAX_PROVENANCE_INTERVALS = 12;
export const DEFAULT_SOURCE_LIMITS = Object.freeze({
  bytes: 2 * 1024 * 1024,
  work: 250_000,
});

const UNKNOWN = Object.freeze({ kind: "unknown" });

/** Keep only explicit, versioned content-authorship evidence. */
export function normalizeAuthorship(evidence) {
  if (!evidence || typeof evidence !== "object") return UNKNOWN;
  if (evidence.kind === "multiple") return { kind: "multiple" };
  if (evidence.kind !== "single") return UNKNOWN;
  const name = typeof evidence.name === "string" ? evidence.name.trim() : "";
  return name ? { kind: "single", name } : UNKNOWN;
}

/**
 * Read only the explicit session fields reserved for content provenance.
 * `by`, `publisher`, and similar event fields are deliberately ignored.
 */
export function captureIntervalAuthorship(session) {
  if (!session || typeof session !== "object") return UNKNOWN;
  return normalizeAuthorship(session.intervalAuthorship || session.authorship);
}

/** Validate the complete, original-adjacent retained interval chain. */
export function provenIntervals(
  checkpoints = [],
  baselineSha,
  targetSha,
  { maxIntervals = MAX_PROVENANCE_INTERVALS } = {},
) {
  if (!targetSha || !Array.isArray(checkpoints)) return null;
  const intervalLimit = Math.min(MAX_PROVENANCE_INTERVALS, Math.max(0, Number(maxIntervals) || 0));
  const start = checkpoints.findIndex((point) => point?.sha === baselineSha);
  const end = checkpoints.findIndex((point) => point?.sha === targetSha);
  if (start < 0 || end <= start) return null;
  const interval = checkpoints.slice(start + 1, end + 1);
  if (!interval.length || interval.length > intervalLimit) return null;
  const result = [];
  let previous = baselineSha;
  for (const point of interval) {
    if (!point || point.ancestry_gap !== false || point.original_parent !== previous) return null;
    const authorship = normalizeAuthorship(point.authorship);
    if (authorship.kind === "unknown") return null;
    result.push({ sha: point.sha, original_parent: point.original_parent, ancestry_gap: false, authorship });
    previous = point.sha;
  }
  return result;
}

function number(value) {
  return Number.isFinite(value) ? Number(value) : null;
}

function validRange(range) {
  if (!range || range.validated !== true || typeof range.path !== "string") return null;
  const start = number(range.start);
  const end = number(range.end);
  if (start === null || end === null || start < 0 || end <= start) return null;
  return { path: range.path, start, end };
}

function rangesFor(value) {
  if (!Array.isArray(value)) return [];
  return value.map(validRange).filter(Boolean);
}

function contributorFor(authorship) {
  if (authorship.kind === "multiple") return "several people";
  return authorship.kind === "single" ? authorship.name : null;
}

// Exact coverage only. A largest-overlap winner would turn an unmapped tail
// or an ambiguous overlap into a false author claim.
function covers(targets, candidates) {
  if (!targets.length || !candidates.length) return null;
  const contributors = new Set();
  for (const target of targets) {
    const boundaries = new Set([target.start, target.end]);
    const matching = candidates.filter((candidate) => {
      if (candidate.path !== target.path || candidate.end <= target.start || candidate.start >= target.end) return false;
      boundaries.add(Math.max(target.start, candidate.start));
      boundaries.add(Math.min(target.end, candidate.end));
      return true;
    });
    const sorted = [...boundaries].sort((a, b) => a - b);
    for (let index = 0; index < sorted.length - 1; index += 1) {
      const start = sorted[index];
      const end = sorted[index + 1];
      if (end <= start) continue;
      const active = matching.filter((candidate) => candidate.start <= start && candidate.end >= end);
      if (!active.length) return null;
      for (const candidate of active) contributors.add(candidate.contributor);
    }
  }
  if (!contributors.size) return null;
  if (contributors.has("several people") || contributors.size > 1) return "several people";
  return [...contributors][0];
}

/**
 * Refine rendered hunks from validated source ranges, without rendering any
 * intermediate checkpoint. Each interval has `sourceRanges` and explicit
 * `authorship`; each hunk may have validated `sourceRanges`. Whole-hunk
 * coverage is required, so partial or ambiguous mappings remain unknown.
 */
export function refineSourceAttribution({
  checkpoints = [], baselineSha, targetSha, intervals = [], hunks = [], limits = {},
} = {}) {
  const sourceLimits = { ...DEFAULT_SOURCE_LIMITS, ...limits };
  if (!Array.isArray(intervals) || !Array.isArray(hunks)) {
    return { hunks: [], refined: false, reason: "invalid-input" };
  }
  const chain = provenIntervals(checkpoints, baselineSha, targetSha);
  if (!chain || intervals.length !== chain.length || intervals.length > MAX_PROVENANCE_INTERVALS) {
    return { hunks: hunks.map((hunk) => ({ ...hunk })), refined: false, reason: "unknown-interval" };
  }
  let bytes = 0;
  let work = 0;
  const candidates = [];
  for (let index = 0; index < intervals.length; index += 1) {
    const interval = intervals[index];
    const expected = chain[index];
    if (!interval || interval.sha !== expected.sha || interval.original_parent !== expected.original_parent) {
      return { hunks: hunks.map((hunk) => ({ ...hunk })), refined: false, reason: "unknown-interval" };
    }
    const authorship = normalizeAuthorship(interval.authorship);
    const contributor = contributorFor(authorship);
    const ranges = rangesFor(interval.sourceRanges);
    bytes += Math.max(0, Number(interval.sourceBytes) || 0);
    work += Math.max(ranges.length, Number(interval.diffWork) || 0);
    if (!contributor || !ranges.length) {
      return { hunks: hunks.map((hunk) => ({ ...hunk })), refined: false, reason: "unknown-authorship" };
    }
    if (bytes > sourceLimits.bytes || work > sourceLimits.work) {
      return { hunks: hunks.map((hunk) => ({ ...hunk })), refined: false, reason: "source-limit" };
    }
    candidates.push(...ranges.map((range) => ({ ...range, contributor })));
  }
  let changed = false;
  const refined = hunks.map((hunk) => {
    const who = covers(rangesFor(hunk?.sourceRanges), candidates);
    if (!who) return { ...hunk };
    changed = true;
    return { ...hunk, who };
  });
  return { hunks: refined, refined: changed, reason: changed ? "mapped" : "unmapped" };
}
