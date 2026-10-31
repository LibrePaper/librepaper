import { getPrivate, SHELL_HEADERS } from "./api.js";
import { m } from "./paraglide/messages.js";
import { locale } from "./locale.js";

export const loadStorageStatus = () => getPrivate("/api/account/storage");

export const trimHistory = (slug) =>
  fetch(`/api/documents/${encodeURIComponent(slug)}/history/trim`, {
    method: "POST",
    headers: { "content-type": "application/json", ...SHELL_HEADERS },
    body: "{}",
  })
    .then(async (response) => {
      const body = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(body.error || `${response.status}`);
      return body;
    });

export function storageBytes(value) {
  if (!Number.isFinite(value) || value < 0) return m.quota_preferences_unavailable();
  const units = [m.quota_preferences_unit_b(), m.quota_preferences_unit_kib(), m.quota_preferences_unit_mib(), m.quota_preferences_unit_gib(), m.quota_preferences_unit_tib()];
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toLocaleString(locale(), { maximumFractionDigits: unit ? 1 : 0 })} ${units[unit]}`;
}
