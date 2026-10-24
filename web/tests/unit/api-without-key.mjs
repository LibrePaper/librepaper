import assert from "node:assert/strict";
import { withoutKey } from "../../src/lib/api.js";

assert.equal(withoutKey("https://app.example/d/paper#k=secret"), "https://app.example/d/paper");
assert.equal(withoutKey("https://app.example/d/paper?v=1#k=secret&x=1"), "https://app.example/d/paper?v=1");
assert.equal(withoutKey("https://app.example/d/paper"), "https://app.example/d/paper");
assert.equal(withoutKey("https://app.example/docs/paper/secret"), "https://app.example/docs/paper");
assert.equal(withoutKey("https://app.example/docs/paper/secret?v=1#k=x"), "https://app.example/docs/paper?v=1");
