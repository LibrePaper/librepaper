// Read by both the Vite plugin and `paraglide-js compile`, so the build and
// the tests compile the messages the same way.
//
// The locale is a setting of this browser, never part of an address: a saved
// choice wins, then the browser's own languages, then English.
import { defineConfig } from "@inlang/paraglide-js";

export default defineConfig({
  outdir: "./src/lib/paraglide",
  strategy: ["localStorage", "preferredLanguage", "baseLocale"],
});
