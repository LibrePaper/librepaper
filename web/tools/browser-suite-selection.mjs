import { basename } from "node:path";

/// Browser-suite invocations stay Chromium-only even when a standalone test
/// supports additional browser selections.
export function browserSuiteArguments(testPath) {
  return basename(testPath) === "latex-browser.mjs" ? ["chromium"] : [];
}

if (process.argv[1] && basename(process.argv[1]) === basename(import.meta.filename)) {
  for (const argument of browserSuiteArguments(process.argv[2] || "")) console.log(argument);
}
