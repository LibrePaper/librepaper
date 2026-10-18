// Recent and favorite symbols for the picker. Ids, not glyphs, are stored, so a
// symbol keeps its per-format spelling.
import { read, write, SYMBOLS_RECENT, SYMBOLS_FAVORITES } from "../storage.js";

export const RECENT_LIMIT = 24;
export const FAVORITES_LIMIT = 200;

const idsFrom = (value, limit) => {
  if (!Array.isArray(value)) return [];
  return [...new Set(value.filter(item => typeof item === "string"))].slice(0, limit);
};

/** @returns {string[]} */
export function readRecent() {
  return idsFrom(read(SYMBOLS_RECENT, []), RECENT_LIMIT);
}

/** @returns {string[]} */
export function readFavorites() {
  return idsFrom(read(SYMBOLS_FAVORITES, []), FAVORITES_LIMIT);
}

/**
 * @param {string[]} ids
 * @param {string} id the symbol just used
 * @returns {string[]}
 */
export function withRecent(ids, id) {
  return [id, ...ids.filter(item => item !== id)].slice(0, RECENT_LIMIT);
}

/**
 * @param {string[]} ids
 * @param {string} id
 * @param {boolean} on true to star the symbol, false to unstar it
 * @returns {string[]}
 */
export function withFavorite(ids, id, on) {
  if (!on) return ids.filter(item => item !== id);
  if (ids.includes(id)) return [...ids];
  return [...ids, id].slice(0, FAVORITES_LIMIT);
}

/** @param {string[]} ids */
export function saveRecent(ids) {
  write(SYMBOLS_RECENT, ids.slice(0, RECENT_LIMIT));
}

/** @param {string[]} ids */
export function saveFavorites(ids) {
  write(SYMBOLS_FAVORITES, ids.slice(0, FAVORITES_LIMIT));
}
