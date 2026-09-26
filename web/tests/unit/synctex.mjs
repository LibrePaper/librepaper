import assert from "node:assert/strict";
import { gzipSync } from "node:zlib";
import { lineAt, parse } from "../../src/lib/synctex.js";

const fixture = `SyncTeX Version:1
Input:1:/work/main.tex
Input:2:/work/chapters/one.tex
Input:3:/texmf/article.cls
Output:pdf
Magnification:1000
Unit:1
X Offset:0
Y Offset:0
Content:
{1
h1,3:6578176,13156352:3289088,657818,0
h2,8:13156352,26312704:657818,657818,0
h3,4:1,1:1,1,0
}1
{2
h1,10:19734528,32890880:657818,657818,0
}2
Postamble:
Count:3
`;

const index = await parse(new Uint8Array(gzipSync(fixture)), ["main.tex", "chapters/one.tex"]);
assert.ok(index);
const forward = index.forward("main.tex", 3);
assert.deepEqual({ path: forward.path, line: forward.line, page: forward.page },
  { path: "main.tex", line: 3, page: 1 });
assert.ok(Math.abs(forward.x - 100) < 0.001 && Math.abs(forward.y - 190) < 0.001);
assert.ok(Math.abs(forward.width - 50) < 0.001 && Math.abs(forward.height - 10) < 0.001);
assert.equal(index.forward("main.tex", 9).page, 2);
assert.equal(index.inverse(1, 201, 395).path, "chapters/one.tex");
assert.equal(index.inverse(1, 201, 395).line, 8);
assert.equal(index.inverse(2, 301, 495).line, 10);
assert.equal(await parse(new TextEncoder().encode("not synctex"), ["main.tex"]), null);
assert.equal(lineAt("one\ntwo\nthree", 0), 1);
assert.equal(lineAt("one\ntwo\nthree", 5), 2);
assert.equal(lineAt("one\ntwo\nthree", 999), 3);

console.log("synctex: gzip parsing, path normalization and both spatial queries pass");
