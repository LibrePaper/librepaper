import assert from "node:assert/strict";
import { parseDelimited, textPage } from "../../src/lib/text-page.js";

// --- parseDelimited: simple rows ----------------------------------------------

assert.deepEqual(parseDelimited("a,b\nc,d", ","), [["a", "b"], ["c", "d"]], "simple rows");

// --- a quoted field may hold the delimiter ------------------------------------

assert.deepEqual(parseDelimited("\"x,y\",z", ","), [["x,y", "z"]], "quoted delimiter is literal");

// --- a doubled quote is one literal quote -------------------------------------

assert.deepEqual(parseDelimited("\"say \"\"hi\"\"\",2", ","), [["say \"hi\"", "2"]], "doubled quote is literal");

// --- a quoted field may hold a line break -------------------------------------

assert.deepEqual(parseDelimited("\"one\ntwo\",3\n", ","), [["one\ntwo", "3"]], "quoted newline is literal");

// --- CRLF line endings --------------------------------------------------------

assert.deepEqual(parseDelimited("a,b\r\nc,d\r\n", ","), [["a", "b"], ["c", "d"]], "CRLF rows");

// --- a trailing line break adds no empty row ----------------------------------

assert.equal(parseDelimited("a,b\n", ",").length, 1, "trailing newline gives no extra row");
assert.equal(parseDelimited("a,b", ",").length, 1, "unterminated last line is still a row");

// --- a leading byte order mark is stripped ------------------------------------

assert.deepEqual(parseDelimited("﻿name,age\n", ","), [["name", "age"]], "BOM stripped");

// --- the tab delimiter ----------------------------------------------------------

assert.deepEqual(parseDelimited("a\tb,c\n1\t2", "\t"), [["a", "b,c"], ["1", "2"]], "tab delimiter");

// --- a malformed quote does not throw -----------------------------------------

assert.doesNotThrow(() => parseDelimited("\"a\"b,c\n\"unclosed", ","), "malformed quotes do not throw");
assert.deepEqual(parseDelimited("\"a\"b,c", ","), [["ab", "c"]], "text after a closing quote is kept");

// --- textPage: a CSV file is a table ------------------------------------------

let page = await textPage("name,value\n<b>x</b>,12.5\n", "data.csv", "data.csv");
assert.match(page, /<table/, "CSV renders a table");
assert.match(page, /<th>name<\/th>/, "the first row is the header");
assert.match(page, /&lt;b&gt;x&lt;\/b&gt;/, "cells are escaped");
assert.match(page, /<td class="num">12\.5<\/td>/, "numbers are marked");
assert.doesNotMatch(page, /<script/i, "the table page has no script");

// --- textPage: a TSV file is a table split on tabs ----------------------------

page = await textPage("a\tb\n1\t2\n", "data.tsv", "data.tsv");
assert.match(page, /<th>a<\/th><th>b<\/th>/, "TSV header split on tabs");

// --- textPage: short rows are padded to the widest row ------------------------

page = await textPage("a,b,c\n1\n", "data.csv", "data.csv");
assert.equal((page.match(/<td/g) || []).length, 3, "a short row is padded to three cells");

// --- textPage: more than 5,000 body rows are cut with a note ------------------

const lines = ["id,value"];
for (let i = 0; i < 5001; i += 1) lines.push(`${i},x`);
page = await textPage(lines.join("\n"), "big.csv", "big.csv");
assert.match(page, /Showing the first 5,000 of 5,001 rows\./, "long tables say how many rows are shown");
assert.equal((page.match(/<tr>/g) || []).length, 5001, "header plus 5,000 body rows");

console.log("text page tests passed");
