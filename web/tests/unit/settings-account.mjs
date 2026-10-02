import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const read = (name) => readFile(new URL(`../../src/components/settings/${name}`, import.meta.url), "utf8");
const registry = await read("registry.js");
const account = await read("AccountSettings.svelte");
const dialog = await read("SettingsDialog.svelte");
const api = await readFile(new URL("../../src/lib/api.js", import.meta.url), "utf8");
const { offered, CATEGORIES } = await import("../../src/components/settings/registry.js");

// The account is the deployment's rather than the document's: it is offered
// to everybody because the server it describes is everybody's, but erasure and
// storage are offered only to a signed-in browser, whatever is open; a gate
// that slipped to `mayEdit` or to a format would hide the erasure surface from
// exactly the readers most likely to want it -- somebody who only ever
// commented on other people's documents.
assert.match(registry, /const account = \(\{ signedIn \}\) => Boolean\(signedIn\);/);
assert.match(registry, /id: "account", says: "Account", offered: always,/);
assert.match(registry, /id: "account-erase",.*offered: account }/);
assert.match(dialog, /\{#if row\("account-erase"\)\}<AccountSettings \{account\} \/>\{\/if\}/);
assert.match(dialog, /signedIn: Boolean\(account\.provider\)/);
assert.match(dialog, /shown\.id === "account"/);

// Erasure is one request and it is the last one this browser makes as that
// account, so it goes through the shared helper that carries the shell
// header. A bare fetch here would be refused as cross-site.
assert.match(api, /export const eraseAccount = \(\) => post\("\/api\/account\/erase"\);/);
assert.match(account, /import \{ eraseAccount \} from "\.\.\/\.\.\/lib\/api\.js";/);
assert.doesNotMatch(account, /fetch\(/);

// Behavior checks: offered category and rows by context.
const accountCategory = CATEGORIES.find((category) => category.id === "account");
const accountRows = (context) => accountCategory.entries.filter((entry) => !entry.offered || entry.offered(context)).map((entry) => entry.id);
// Unsigned browsers see remote-status but no erasure.
const unsigned = accountRows({ signedIn: false, format: "", mayEdit: false });
assert.ok(unsigned.includes("account"), "Account category is offered to unsigned browsers");
assert.deepEqual(unsigned, ["remote-status"], "Unsigned browser sees only remote-status");
// Signed-in browsers see erasure and storage.
const signed = accountRows({ signedIn: true, format: "", mayEdit: false });
assert.ok(signed.includes("account-erase"), "Signed-in browser includes account-erase");
assert.ok(signed.includes("storage-account"), "Signed-in browser includes storage-account");

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
