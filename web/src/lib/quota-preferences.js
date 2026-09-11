import { getPrivate, post } from "./api.js";

export const loadQuotaPreferences = () => getPrivate("/api/account/storage").then(storageSnapshot);
export const previewQuotaPreferences = (revision, preferences) =>
  post("/api/account/storage/preview", { revision, preferences });
export const applyQuotaPreferences = async (revision, generation, preferences, confirmed = false) => {
  const result = await post("/api/account/storage/apply", { revision, generation, preferences, confirmed });
  globalThis.window?.dispatchEvent(new Event("librepaper-quota-preferences"));
  return result;
};

export function storageBytes(value) {
  if (!Number.isFinite(value) || value < 0) return "Unavailable";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) { size /= 1024; unit++; }
  return `${size.toLocaleString(undefined, { maximumFractionDigits: unit ? 1 : 0 })} ${units[unit]}`;
}

// Timezone affects presentation only. The server supplies every retention tier.
export function displayTimestamp(value, timezone = "UTC") {
  const date = new Date(typeof value === "number" ? value * 1000 : value);
  if (!Number.isFinite(date.getTime())) return "Unavailable";
  const options = { dateStyle: "medium", timeStyle: "short", timeZone: timezone || "UTC" };
  try { return new Intl.DateTimeFormat(undefined, options).format(date); }
  catch { return new Intl.DateTimeFormat(undefined, { ...options, timeZone: "UTC" }).format(date); }
}

export function validateTimezone(value) {
  if (!value || value.length > 100) return false;
  try { new Intl.DateTimeFormat("en", { timeZone: value }).format(0); return true; }
  catch { return false; }
}

export const MILESTONES = [
  ["named", "Named versions"], ["cli", "Explicit command-line milestones"],
  ["publish", "Publications"], ["restore", "Restored versions"],
  ["accept", "Accepted suggestions"], ["comment", "Versions referenced by open comments and suggestions"],
];

const PROFILE_LABELS = {
  balanced: "Balanced", moreRecoveryPoints: "More recovery points",
  useLessStorage: "Use less storage", custom: "Custom",
};

function duration(seconds) {
  for (const [size, unit] of [[86400, "day"], [3600, "hour"], [60, "minute"]]) {
    if (seconds >= size && seconds % size === 0) return `${seconds / size} ${unit}${seconds === size ? "" : "s"}`;
  }
  return `${seconds} seconds`;
}

export function displayTiers(tiers = []) {
  let lower = 0;
  return tiers.map((tier) => {
    const upper = tier.maxAgeSeconds;
    const ageLabel = upper == null ? `${duration(lower)} and older`
      : lower ? `${duration(lower)} to less than ${duration(upper)}` : `Less than ${duration(upper)}`;
    lower = upper;
    return { ...tier, ageLabel, densityLabel: `Newest per ${duration(tier.bucketSeconds)} UTC bucket` };
  });
}

export function storageSnapshot(result) {
  const bounds = result.constraints || {};
  const use = result.usage || {};
  const fraction = bounds.hardQuotaBytes > 0 ? 100 * use.chargedBytes / bounds.hardQuotaBytes : 0;
  const threshold = (result.preferences?.warningThresholds || []).filter((value) => fraction >= value).at(-1);
  const thinning = result.thinning || {};
  const statuses = {
    grace: `A retention change is scheduled after ${displayTimestamp(thinning.graceUntil, result.preferences?.displayTimezone)}.`,
    pending: "Routine history thinning is queued; live saving continues.",
    running: "Routine history thinning is in progress; live saving continues.",
    complete: "The latest history thinning pass is complete. Physical cleanup may still be pending.",
    stale: "An older thinning plan was superseded by a newer policy.",
  };
  return {
    ...result,
    canManage: result.canManage !== false && !result.effective?.incompatible,
    effective: { ...result.effective, tiers: displayTiers(result.effective?.retention?.tiers) },
    profiles: (result.profiles || []).filter((profile) => profile !== "custom" || result.preferences?.retentionProfile === "custom")
      .map((profile) => typeof profile === "string" ? { id: profile, label: PROFILE_LABELS[profile] || profile } : profile),
    usage: {
      ...use,
      message: use.physicalAccounting === true && threshold != null
        ? `Storage use has reached the ${threshold}% warning threshold of your hard quota.${fraction >= 100 ? " New writes or artifacts may be restricted." : ""}` : "",
      authoritative: use.physicalAccounting === true,
      hardQuota: bounds.hardQuotaBytes,
      categories: [
        { id: "live", label: "Live document state and source", bytes: use.liveBytes },
        { id: "history", label: "Source history", bytes: use.sourceHistoryBytes },
        { id: "assets", label: "Assets", bytes: use.assetBytes },
        { id: "metadata", label: "Other durable metadata", bytes: use.metadataBytes },
      ],
    },
    thinning: { ...thinning, message: statuses[thinning.status] || "" },
    constraints: [
      { explanation: `Hard storage quota: ${storageBytes(bounds.hardQuotaBytes)}.` },
      ...(bounds.minimumBucketSeconds ? [{ explanation: `Minimum supported bucket width: ${duration(bounds.minimumBucketSeconds)}.` }] : []),
      ...(bounds.maximumRetentionSeconds ? [{ explanation: `Maximum retention duration: ${duration(bounds.maximumRetentionSeconds)}.` }] : []),
      ...(bounds.maxCheckpointCount != null ? [{ explanation: `Hard checkpoint count: ${Math.max(1, bounds.maxCheckpointCount)} per document, including the newest version.` }] : []),
      ...(result.effective?.retention?.explanation || []).map((explanation) => ({ explanation })),
    ],
  };
}

export function draftPreferences(saved, { budgetKind, budgetValue, profile, timezone, milestones, warnings }) {
  const value = Number(budgetValue);
  if (!Number.isFinite(value) || value < 0 || (budgetKind === "percent" && (!Number.isInteger(value) || value > 100))) {
    throw new Error("Enter a nonnegative history budget; a percentage must be a whole number from 0 to 100.");
  }
  if (!["bytes", "percent"].includes(budgetKind)) throw new Error("Choose a budget unit.");
  if (!validateTimezone(timezone)) throw new Error("Enter a recognized timezone, such as America/Toronto or UTC.");
  const thresholds = String(warnings).split(",").map((part) => Number(part.trim()));
  if (!thresholds.length || thresholds.length > 4 || thresholds.some((number, index) => !Number.isInteger(number) || number < 1 || number > 100 || (index && number <= thresholds[index - 1]))) {
    throw new Error("Warning percentages must increase, separated by commas, between 1 and 100.");
  }
  // Preserve fields the form does not edit, including document overrides.
  return {
    ...saved,
    historyBudget: { kind: budgetKind, value: budgetKind === "bytes" ? Math.round(value * 1024 * 1024) : value },
    retentionProfile: profile,
    customRetention: profile === "custom" ? saved.customRetention : null,
    displayTimezone: timezone,
    milestonePreferences: { ...saved.milestonePreferences, ...milestones },
    warningThresholds: thresholds,
  };
}
