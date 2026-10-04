// A command typed in a settings field becomes the argv the companion runs,
// and a saved argv shows again as the same command.

import assert from "node:assert/strict";
import { joinArgs, splitArgs } from "../../src/lib/companion/args.js";

// --- splitting ---------------------------------------------------------------

assert.deepEqual(splitArgs("npx -y @agent/acp"), ["npx", "-y", "@agent/acp"]);
assert.deepEqual(splitArgs("  a \t b\nc  "), ["a", "b", "c"]);
assert.deepEqual(splitArgs(`--metadata "title=Two words"`), ["--metadata", "title=Two words"]);
assert.deepEqual(splitArgs(`'single $quoted' x`), ["single $quoted", "x"]);
assert.deepEqual(splitArgs("/opt/My\\ Tools/agent --stdio"), ["/opt/My Tools/agent", "--stdio"]);
assert.deepEqual(splitArgs(`say \\"hi\\"`), ["say", `"hi"`]);
assert.deepEqual(splitArgs(`"a \\"b\\" c"`), [`a "b" c`]);
assert.deepEqual(splitArgs("\\\\server\\share\\agent.exe --root \\\\server\\data"), ["\\\\server\\share\\agent.exe", "--root", "\\\\server\\data"]);
assert.deepEqual(splitArgs(`"\\\\server\\My Share\\agent.exe"`), ["\\\\server\\My Share\\agent.exe"]);
assert.deepEqual(splitArgs("C:\\Users\\me\\agent.exe --acp"), ["C:\\Users\\me\\agent.exe", "--acp"]);
assert.deepEqual(splitArgs(`"C:\\Program Files\\agent.exe"`), ["C:\\Program Files\\agent.exe"]);
assert.deepEqual(splitArgs(`x "" y`), ["x", "", "y"]);
assert.deepEqual(splitArgs(""), []);

// --- round trip --------------------------------------------------------------

for (const argv of [
  ["plain", "--flag"],
  ["two words", "tab\there", "new\nline"],
  [`quote"inside`, "it's", "back\\slash", "C:\\Program Files\\q.exe"],
  ["", "after-empty"],
  ["\\\\server\\share\\a.exe", "\\\\server\\My Share", "trailing\\"],
]) {
  assert.deepEqual(splitArgs(joinArgs(argv)), argv, JSON.stringify(argv));
}
assert.equal(joinArgs(["a", "b c", "it's"]), `a 'b c' 'it'\\''s'`);
assert.equal(joinArgs(null), "");

console.log("args: terminal-style splitting, escaped whitespace, Windows paths and round trips passed");
