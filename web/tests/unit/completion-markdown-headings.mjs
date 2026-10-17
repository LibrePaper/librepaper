// Heading ids with code spans. Code content is kept as written, while the
// other reductions still apply to the text around it.
import assert from "node:assert/strict";
import { markdownLabels } from "../../src/lib/completion/markdown.js";

const ids = (text) => markdownLabels({ "doc.md": text }).map((label) => label.id);

// Code spans keep their underscores.
assert.deepEqual(ids("# The `__init__` method"), ["the-__init__-method"]);
// Angle brackets inside code are kept, then dropped by the slug.
assert.deepEqual(ids("# Using `<div>` tags"), ["using-div-tags"]);
// Emphasis outside code still goes.
assert.deepEqual(ids("# The **bold** `code` move"), ["the-bold-code-move"]);

// Earlier cases still hold.
assert.deepEqual(ids("# Using `npm`"), ["using-npm"]);
assert.deepEqual(ids("# [Intro](other.md)"), ["intro"]);
assert.deepEqual(ids("# __init__ file"), ["init-file"]);
