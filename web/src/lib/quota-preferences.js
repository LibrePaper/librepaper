import { getPrivate } from "./api.js";

export const loadStorageStatus = () => getPrivate("/api/account/storage");

export function storageBytes(value) {
  if (!Number.isFinite(value) || value < 0) return "Unavailable";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toLocaleString(undefined, { maximumFractionDigits: unit ? 1 : 0 })} ${units[unit]}`;
}
