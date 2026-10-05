import { accessSync, constants, statSync } from "node:fs";
import { delimiter, isAbsolute, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const CHROMIUM_NAMES = ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"];
const CHROMIUM_ENV = ["LIBREPAPER_CHROMIUM", "CHROMIUM_BIN", "CHROME_BIN"];

function executable(candidate, pathValue = process.env.PATH || "") {
  if (!candidate) return null;
  const paths = candidate.includes("/") || candidate.includes("\\") || isAbsolute(candidate)
    ? [resolve(candidate)]
    : pathValue.split(delimiter).filter(Boolean).map((directory) => join(directory, candidate));
  for (const path of paths) {
    try {
      accessSync(path, constants.X_OK);
      if (statSync(path).isFile()) return resolve(path);
    } catch {}
  }
  return null;
}

/// The one Chromium binary used by suite launchers and browser fixtures.
/// An explicit environment setting is authoritative: a typo must fail rather
/// than silently choosing another browser from PATH.
export function resolveChromiumExecutable(env = process.env) {
  const explicitName = CHROMIUM_ENV.find((name) => env[name]);
  if (explicitName) {
    const path = executable(env[explicitName], env.PATH);
    if (!path) throw new Error(`${explicitName} does not name an executable: ${env[explicitName]}`);
    return path;
  }
  for (const candidate of CHROMIUM_NAMES) {
    const path = executable(candidate, env.PATH);
    if (path) return path;
  }
  return null;
}

export function requireChromiumExecutable(env = process.env) {
  const path = resolveChromiumExecutable(env);
  if (!path) throw new Error("No Chromium executable found; install chromium or set LIBREPAPER_CHROMIUM to its executable path.");
  return path;
}

const invokedPath = process.argv[1] && pathToFileURL(resolve(process.argv[1])).href;
if (invokedPath === import.meta.url && process.argv.includes("--print")) {
  try {
    const path = resolveChromiumExecutable();
    if (!path) {
      console.error("No Chromium executable found; install chromium or set LIBREPAPER_CHROMIUM to its executable path.");
      process.exitCode = 1;
    } else {
      console.log(path);
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
