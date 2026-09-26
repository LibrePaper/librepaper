import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { chmodSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const here = dirname(fileURLToPath(import.meta.url));
const installer = join(here, "../deploy/install.sh");

function sandbox(t, exitCode = 0) {
  const root = mkdtempSync(join(tmpdir(), "librepaper-install-wrapper-"));
  const bin = join(root, "bin");
  const curlLog = join(root, "curl.log");
  const result = join(root, "installer-result");
  const stub = join(root, "stub-installer.sh");
  mkdirSync(bin);
  writeFileSync(stub, `#!/bin/sh
printf '%s\\n' "\${LIBREPAPER_VERSION-}" "\${LIBREPAPER_INSTALL_DIR-}" "$@" > "$INSTALLER_RESULT"
exit "\${STUB_EXIT:-0}"
`);
  const transport = `#!/bin/sh
for url do :; done
printf '%s\\n' "$url" >> "$CURL_LOG"
case "$url" in
  */librepaper-installer.sh) cat "$STUB_INSTALLER" ;;
  *) printf '%s\\n' 'unexpected URL' >&2; exit 42 ;;
esac
`;
  for (const name of ["curl", "wget"]) {
    const path = join(bin, name);
    writeFileSync(path, transport);
    chmodSync(path, 0o755);
  }
  chmodSync(stub, 0o755);
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return {
    result,
    curlLog,
    env: {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      CURL_LOG: curlLog,
      STUB_INSTALLER: stub,
      INSTALLER_RESULT: result,
      STUB_EXIT: String(exitCode),
    },
  };
}

function run(wrapper, args, env) {
  return execFileSync("/bin/sh", [wrapper, ...args], { env, encoding: "utf8" });
}

test("legacy install.sh forwards latest requests to Cargo Dist and maps the old install directory", (t) => {
  const box = sandbox(t);
  const env = { ...box.env, LIBREPAPER_BIN_DIR: "/tmp/legacy-bin", LIBREPAPER_INSTALL_DIR: "/tmp/new-bin" };
  run(installer, ["--no-confirm", "value with spaces"], env);

  assert.equal(
    readFileSync(box.curlLog, "utf8").trim(),
    "https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.sh",
  );
  assert.deepEqual(
    readFileSync(box.result, "utf8").trimEnd().split("\n"),
    ["", "/tmp/legacy-bin", "--no-confirm", "value with spaces"],
  );
});

test("legacy install.sh forwards a pinned release and preserves LIBREPAPER_INSTALL_DIR", (t) => {
  const box = sandbox(t);
  const env = { ...box.env, LIBREPAPER_VERSION: "v1.2.3", LIBREPAPER_INSTALL_DIR: "/tmp/custom-bin" };
  run(installer, ["--quiet"], env);

  assert.equal(
    readFileSync(box.curlLog, "utf8").trim(),
    "https://github.com/LibrePaper/librepaper/releases/download/v1.2.3/librepaper-installer.sh",
  );
  assert.deepEqual(
    readFileSync(box.result, "utf8").trimEnd().split("\n"),
    ["v1.2.3", "/tmp/custom-bin", "--quiet"],
  );
});

test("legacy install.sh propagates a generated installer failure status", (t) => {
  const box = sandbox(t, 37);
  assert.throws(
    () => run(installer, [], box.env),
    (error) => error.status === 37,
  );
});
