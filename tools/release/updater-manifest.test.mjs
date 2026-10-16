// Offline checks of latest.json generation. Run by tools/release/packaging.test.sh.
import assert from "node:assert/strict";
import { manifest } from "./updater-manifest.mjs";

const pubDate = "2026-10-15T00:00:00.000Z";
const signatures = { windows: "windows-signature", macos: "macos-signature" };

const stable = manifest({ tag: "v1.2.3", version: "1.2.3", signatures, pubDate });
const base = "https://github.com/LibrePaper/librepaper/releases/download/v1.2.3";
assert.equal(stable.version, "1.2.3");
assert.equal(stable.pub_date, pubDate);
assert.deepEqual(stable.platforms["windows-x86_64"], {
  signature: "windows-signature",
  url: `${base}/LibrePaper_1.2.3_x64-setup.exe`,
});
assert.deepEqual(stable.platforms["darwin-aarch64"], {
  signature: "macos-signature",
  url: `${base}/LibrePaper_1.2.3_universal.app.tar.gz`,
});
assert.deepEqual(stable.platforms["darwin-x86_64"], {
  signature: "macos-signature",
  url: `${base}/LibrePaper_1.2.3_universal.app.tar.gz`,
});

const pre = manifest({ tag: "v1.2.3-rc.1", version: "1.2.3-rc.1", signatures, pubDate });
const preBase = "https://github.com/LibrePaper/librepaper/releases/download/v1.2.3-rc.1";
assert.equal(pre.platforms["windows-x86_64"].url, `${preBase}/LibrePaper_1.2.3-rc.1_x64-setup.exe`);
assert.equal(pre.platforms["darwin-aarch64"].url, `${preBase}/LibrePaper_1.2.3-rc.1_universal.app.tar.gz`);

assert.throws(
  () => manifest({ tag: "v1.2.3", version: "1.2.4", signatures, pubDate }),
  /does not match tag/,
);

assert.throws(
  () => manifest({ tag: "v1.2.3", version: "1.2.3", signatures: { windows: "", macos: "macos-signature" }, pubDate }),
  /updater signature is empty/,
);

console.log("updater manifest tests passed");
