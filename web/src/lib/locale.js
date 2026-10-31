// The interface language, and the one place the app formats for it.
//
// Paraglide picks the language once, as the page loads: a choice saved in this
// browser, then the browser's own languages, then English. Changing it reloads
// the page, so nothing has to re-render in place.
//
// Pages on the documents origin (the PDF viewer) cannot read this origin's
// saved choice, and follow the browser's languages instead.
import { getLocale, setLocale, localStorageKey } from "./paraglide/runtime.js";

// Each language is named in itself, so somebody who cannot read the current
// one can still find their own.
export const LANGUAGES = { en: "English", fr: "Français" };

/** The language the interface is showing, such as "en" or "fr". */
export const locale = () => getLocale();

/** "system" when nothing is saved and the browser decides, else the saved language. */
export function chosenLanguage() {
  try {
    return localStorage.getItem(localStorageKey) ? getLocale() : "system";
  } catch {
    return "system";
  }
}

/** Saves the choice and reloads the page into it. */
export function chooseLanguage(next) {
  if (next === "system") {
    try { localStorage.removeItem(localStorageKey); } catch { /* nothing saved to clear */ }
    location.reload();
    return;
  }
  setLocale(next);
}

/** Tells the browser, screen readers and the spellchecker what language the page is in. */
export function markDocumentLanguage() {
  document.documentElement.lang = getLocale();
}
