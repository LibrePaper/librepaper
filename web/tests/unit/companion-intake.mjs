import assert from "node:assert/strict";
import * as local from "../../src/lib/companion/client.js";

const address = "http://127.0.0.1:8763/";

function harness(hash = "") {
  const store = new Map();
  const replaced = [];
  const location = { href: `https://app.example.test/${hash ? `#${hash}` : ""}`, hash: hash ? `#${hash}` : "" };
  local._testing.reset();
  local._testing.inject({
    storage: { getItem: (k) => store.get(k) ?? null, setItem: (k, v) => store.set(k, v), removeItem: (k) => store.delete(k) },
    location: () => location,
    replaceHash: (value) => { replaced.push(value); location.hash = value ? `#${value}` : ""; },
  });
  return { location, replaced };
}

// A Settings request sets the companion address, is reported, and leaves unrelated fragment params alone.
{
  const h = harness(`k=reader-key&settings=local&companion_address=${encodeURIComponent("http://127.0.0.1:9911/")}`);
  assert.equal(local.intake(), true);
  assert.equal(h.location.hash, "#k=reader-key");
  assert.deepEqual(h.replaced, ["k=reader-key"]);
  assert.equal(local.address(), "http://127.0.0.1:9911/");
  assert.equal(local.intake(), false);
}

// Legacy control fields are stripped whatever they say.
{
  const h = harness("settings=local&companion_control=abc&companion_instance=def&keep=yes");
  assert.equal(local.intake(), true);
  assert.equal(h.location.hash, "#keep=yes");
  assert.equal(local.address(), address);
}

// A non-loopback address is stripped and never becomes the companion address.
for (const bad of ["https://attacker.example/", "http://127.0.0.1/", "http://user:pw@127.0.0.1:8763/", "http://127.0.0.1:8763/x", "not a url"]) {
  const h = harness(`settings=local&companion_address=${encodeURIComponent(bad)}&keep=yes`);
  assert.equal(local.intake(), true);
  assert.equal(h.location.hash, "#keep=yes");
  assert.equal(local.address(), address, bad);
}

// A fragment with neither field is left untouched.
{
  const h = harness("k=reader-key");
  assert.equal(local.intake(), false);
  assert.deepEqual(h.replaced, []);
}
