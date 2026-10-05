#!/usr/bin/env node
// Save a synthetic login cookie signed by the source deployment's key before
// backup. The restore verifier reuses these exact bytes after restoring the key.
import { writeFileSync } from "node:fs";
import { sessionCookie } from "../../../web/tests/helpers/deployment.mjs";

const [dataDirectory, accountId, output] = process.argv.slice(2);
if (!dataDirectory || !accountId || !output) {
  console.error("usage: save-session-cookie.mjs <data-directory> <account-id> <output>");
  process.exit(2);
}
const cookie = sessionCookie({ dataDirectory, accountId, handle: "fixture", name: "Fixture" });
writeFileSync(output, cookie, { mode: 0o600 });
