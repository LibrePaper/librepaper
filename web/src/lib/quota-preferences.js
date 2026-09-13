import { getPrivate, post } from "./api.js";

export const loadQuotaPreferences = () => getPrivate("/api/account/storage").then(storageSnapshot);
export const previewQuotaPreferences = (revision, preferences) =>
  post("/api/account/storage/preview", { revision, preferences });
export const applyQuotaPreferences = async (revision, preferences) => {
  const result = await post("/api/account/storage/apply", { revision, preferences });
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

// Timezone affects presentation only. The server supplies the retention policy.
export function displayTimestamp(value, timezone = "UTC") {
  const date = new Date(typeof value === "number" ? value : value);
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

const PROFILE_LABELS = { default: "Newest 50 routine versions within 30 days", manual: "Manual cleanup", custom: "Custom limits" };
export function storageSnapshot(result) {
  const bounds = result.constraints || {}, usage = result.usage || {};
  const fraction = bounds.hardQuotaBytes > 0 ? 100 * usage.chargedBytes / bounds.hardQuotaBytes : 0;
  const threshold = (result.preferences?.warningThresholds || []).filter(value => fraction >= value).at(-1);
  return { ...result, canManage: result.canManage !== false && !result.effective?.incompatible,
    profiles: (result.profiles || []).map(id => ({ id, label: PROFILE_LABELS[id] || id })),
    usage: { ...usage, hardQuota: bounds.hardQuotaBytes, message: threshold == null ? "" : `Storage use has reached ${threshold}% of your quota.` },
  };
}
export function draftPreferences(saved, { profile, timezone, warnings, maxCount, maxDays }) {
  if (!["default", "manual", "custom"].includes(profile)) throw new Error("Choose a retention policy.");
  if (!validateTimezone(timezone)) throw new Error("Enter a recognized timezone, such as America/Toronto or UTC.");
  const thresholds = String(warnings).split(",").map(part => Number(part.trim()));
  if (!thresholds.length || thresholds.length > 4 || thresholds.some((n, i) => !Number.isInteger(n) || n < 1 || n > 100 || (i && n <= thresholds[i - 1]))) throw new Error("Warning percentages must increase from 1 to 100.");
  const count = maxCount == null || maxCount === "" ? null : Number(maxCount);
  const days = maxDays == null || maxDays === "" ? null : Number(maxDays);
  if (profile === "custom" && ((count != null && (!Number.isInteger(count) || count < 0 || count > 4096)) || (days != null && (!Number.isFinite(days) || days < 0 || !Number.isSafeInteger(days * 86400000))))) throw new Error("Enter a count from 0 to 4096 and a nonnegative age, or leave a limit blank.");
  return { version: 2, retentionPolicyVersion: 2, retentionProfile: profile,
    customRetention: profile === "custom" ? { maxRoutineCount: count, maxAgeMs: days == null ? null : days * 86400000 } : null,
    displayTimezone: timezone, warningThresholds: thresholds };
}
