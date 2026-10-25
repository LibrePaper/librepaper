// The Typst command line's log, read into the diagnostics the browser engines
// produce. A port of the rules the desktop app used to apply itself.
import assert from "node:assert/strict";
import test from "node:test";
import { failureReason, parse, stripAnsi } from "../../src/lib/typst-log.js";

test("a location is kept only when it names a project file", () => {
  const [diagnostic] = parse("error: unknown function\n┌─ main.typ:4:7", { paths: ["main.typ"] });
  assert.equal(diagnostic.severity, "error");
  assert.equal(diagnostic.message, "unknown function");
  assert.equal(diagnostic.file, "main.typ");
  assert.equal(diagnostic.line, 4);
  assert.equal(diagnostic.column, 7);
  assert.equal(diagnostic.end_line, 4);
  assert.equal(diagnostic.end_column, 7);
  assert.deepEqual(diagnostic.hints, []);
});

test("a path outside the project is not exposed", () => {
  const [diagnostic] = parse("error: failed\n┌─ /tmp/private.typ:1:1", { paths: ["main.typ"] });
  assert.equal(diagnostic.file, "");
  assert.equal(diagnostic.line, 0);
  assert.equal(diagnostic.column, 0);
});

test("the first location that names a project file wins", () => {
  const log = "error: bad\n┌─ /tmp/other.typ:9:9\n┌─ chapters/one.typ:2:3\n┌─ main.typ:5:1";
  const [diagnostic] = parse(log, { paths: ["main.typ", "chapters/one.typ"] });
  assert.equal(diagnostic.file, "chapters/one.typ");
  assert.equal(diagnostic.line, 2);
});

test("the message is the error line alone, without colours or its label", () => {
  assert.equal(parse("error: unknown function\n┌─ main.typ:4:7")[0].message, "unknown function");
  const quarto = "processing file: x.qmd\n\x1b[91mERROR: Failed to spawn 'Rscript'\x1b[39m\n\n\x1b[33mWARN: Error encountered\x1b[39m";
  assert.equal(failureReason(quarto), "Failed to spawn 'Rscript'");
  assert.equal(failureReason("\x1b[31mError in loadNamespace(x) : no package\nExecution halted"), "Error in loadNamespace(x) : no package");
});

test("a log without an error line has a general message", () => {
  assert.equal(failureReason(""), null);
  assert.equal(failureReason("Rendering...\nWARN: Error encountered when rendering files"), null);
  assert.equal(parse("WARN: something")[0].message, "The Typst command failed.");
});

test("colour codes are stripped and the text kept", () => {
  assert.equal(stripAnsi("\x1b[33mWARN: done\x1b[39m\nInstall with \x1b[1mx\x1b[22m"), "WARN: done\nInstall with x");
});
