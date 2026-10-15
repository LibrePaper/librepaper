import assert from "node:assert/strict";
import * as local from "../../src/lib/desktop/client.js";

function harness(hash = "") {
  const replaced = [];
  const location = { href: `https://app.example.test/${hash ? `#${hash}` : ""}`, hash: hash ? `#${hash}` : "" };
  local._testing.reset();
  local._testing.inject({
    location: () => location,
    replaceHash: (value) => { replaced.push(value); location.hash = value ? `#${value}` : ""; },
  });
  return { location, replaced };
}

// A Settings request is reported and leaves unrelated fragment params alone.
{
  const h = harness("k=reader-key&settings=local");
  assert.equal(local.intake(), true);
  assert.equal(h.location.hash, "#k=reader-key");
  assert.deepEqual(h.replaced, ["k=reader-key"]);
  assert.equal(local.intake(), false);
}

// An address field is not a Settings request and is left alone.
{
  const h = harness("address=http%3A%2F%2F127.0.0.1%3A9911%2F&keep=yes");
  assert.equal(local.intake(), false);
  assert.deepEqual(h.replaced, []);
}

// A fragment with no Settings request is left untouched.
{
  const h = harness("k=reader-key");
  assert.equal(local.intake(), false);
  assert.deepEqual(h.replaced, []);
}

local._testing.reset();
