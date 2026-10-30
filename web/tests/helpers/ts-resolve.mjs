// The visual-markup TypeScript is written for Vite: it imports its siblings
// without an extension and imports types as values. Node's type stripping
// accepts neither, so the unit tests resolve such an import to the .ts file
// beside it and compile .ts files with TypeScript, which drops type imports.
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { fileURLToPath } from "node:url";
import ts from "typescript";

registerHooks({
  resolve(specifier, context, next) {
    try {
      return next(specifier, context);
    } catch (error) {
      if (error?.code !== "ERR_MODULE_NOT_FOUND" || !/^\.{1,2}\//.test(specifier) || /\.[a-z]+$/i.test(specifier)) throw error;
      return next(`${specifier}.ts`, context);
    }
  },
  load(url, context, next) {
    if (!url.startsWith("file:") || !url.endsWith(".ts")) return next(url, context);
    const source = readFileSync(fileURLToPath(url), "utf8");
    const { outputText } = ts.transpileModule(source, {
      fileName: fileURLToPath(url),
      compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
    });
    return { format: "module", source: outputText, shortCircuit: true };
  },
});
