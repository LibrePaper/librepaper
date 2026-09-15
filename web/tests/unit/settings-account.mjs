import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const read = (name) => readFile(new URL(`../../src/components/settings/${name}`, import.meta.url), "utf8");
const registry = await read("registry.js");
const account = await read("AccountSettings.svelte");
const dialog = await read("SettingsDialog.svelte");
const api = await readFile(new URL("../../src/lib/api.js", import.meta.url), "utf8");

// The account is the deployment's rather than the document's: it is offered
// on whatever happens to be open, gated only on being signed in. A gate that
// slipped to `mayEdit` or to a format would hide the erasure surface from
// exactly the readers most likely to want it -- somebody who only ever
// commented on other people's documents.
assert.match(registry, /const account = \(\{ signedIn \}\) => Boolean\(signedIn\);/);
assert.match(registry, /id: "account", says: "Account", offered: account,/);
assert.match(registry, /id: "account-erase"/);
assert.match(dialog, /signedIn: Boolean\(account\.provider\)/);
assert.match(dialog, /shown\.id === "account"/);

// Erasure is one request and it is the last one this browser makes as that
// account, so it goes through the shared helper that carries the shell
// header. A bare fetch here would be refused as cross-site.
assert.match(api, /export const eraseAccount = \(\) => post\("\/api\/account\/erase"\);/);
assert.match(account, /import \{ eraseAccount \} from "\.\.\/\.\.\/lib\/api\.js";/);
assert.doesNotMatch(account, /fetch\(/);

// Typing the handle is the whole of the confirmation, and the button stays
// disabled until it matches. `handle !== ""` is not decoration: without it an
// account whose handle never arrived would confirm on an empty box.
assert.match(account, /typed\.trim\(\)\.toLowerCase\(\) === handle\.toLowerCase\(\) && handle !== ""/);
assert.match(account, /disabled=\{!confirmed \|\| erasing\}/);
assert.match(account, /if \(!confirmed \|\| erasing\) return;/);

// What erasure actually does, in the words of the person it happens to. The
// server keeps comment bodies on other people's documents and relabels their
// author, and it invalidates the session on the spot -- both are surprises
// unless the page says so before the button is pressed, not after.
assert.match(account, /Deleted user/);
assert.match(account, /cannot sign in again/);
