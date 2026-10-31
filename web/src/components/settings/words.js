// Words the settings share: a size a person can read.
import { m } from "../../lib/paraglide/messages.js";

export function megabytes(bytes) {
  if (bytes == null) return "…";
  const mb = bytes / (1024 * 1024);
  return mb < 0.1 ? m.words_nothing() : mb < 10 ? `${mb.toFixed(1)} MB` : `${Math.round(mb)} MB`;
}
