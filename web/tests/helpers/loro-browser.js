// The package's browser build, as the browser tests take it (see loro.mjs),
// plus the one thing loro-web.js does that those tests still need: filling
// the class registry that anchors and track changes read instead of importing
// loro-crdt themselves.
import * as loro from "../../node_modules/loro-crdt/browser/index.js";
import { setLoroClasses } from "../../src/lib/loro-classes.js";

setLoroClasses(loro);

export * from "../../node_modules/loro-crdt/browser/index.js";
