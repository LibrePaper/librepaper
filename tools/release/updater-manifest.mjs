// Builds latest.json for the desktop app's updater from the signatures of one
// release. Usage: node tools/release/updater-manifest.mjs --tag vX.Y.Z --dir DIR
import { readFileSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const TAG_PATTERN = /^v\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/;
const USAGE = "usage: updater-manifest.mjs --tag vX.Y.Z --dir DIR";

export function manifest({ tag, version, signatures, pubDate }) {
  if (!TAG_PATTERN.test(tag)) {
    throw new Error(`Tag ${tag} is not a version tag like v0.0.43.`);
  }
  if (version !== tag.slice(1)) {
    throw new Error(`Version ${version} does not match tag ${tag}.`);
  }
  for (const platform of ["windows", "macos"]) {
    if (typeof signatures[platform] !== "string" || signatures[platform].trim() === "") {
      throw new Error(`The ${platform} updater signature is empty.`);
    }
  }
  const { windows, macos } = signatures;
  const base = `https://github.com/LibrePaper/librepaper/releases/download/${tag}`;
  const macArchive = `${base}/LibrePaper_${version}_universal.app.tar.gz`;
  return {
    version,
    notes: `See https://github.com/LibrePaper/librepaper/releases/tag/${tag}`,
    pub_date: pubDate,
    platforms: {
      "windows-x86_64": {
        signature: windows,
        url: `${base}/LibrePaper_${version}_x64-setup.exe`,
      },
      "darwin-aarch64": {
        signature: macos,
        url: macArchive,
      },
      "darwin-x86_64": {
        signature: macos,
        url: macArchive,
      },
    },
  };
}

function parseArgs(argv) {
  const options = {};
  for (let i = 0; i < argv.length; i += 2) {
    const name = argv[i];
    const value = argv[i + 1];
    if (!["--tag", "--dir"].includes(name) || value === undefined) {
      throw new Error(USAGE);
    }
    options[name.slice(2)] = value;
  }
  if (!options.tag || !options.dir) {
    throw new Error(USAGE);
  }
  return options;
}

function readSignature(file) {
  return readFileSync(file, "utf8").trim();
}

function main(argv) {
  const { tag, dir } = parseArgs(argv);
  const version = tag.replace(/^v/, "");
  const signatures = {
    windows: readSignature(path.join(dir, `LibrePaper_${version}_x64-setup.exe.sig`)),
    macos: readSignature(path.join(dir, `LibrePaper_${version}_universal.app.tar.gz.sig`)),
  };
  const result = manifest({ tag, version, signatures, pubDate: new Date().toISOString() });
  console.log(JSON.stringify(result, null, 2));
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2));
}
