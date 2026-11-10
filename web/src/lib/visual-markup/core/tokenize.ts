import { Language } from './language';
import { Token } from './tokens';

// The tokens of a source, built from the language's shared syntax tree. Null means the
// tree is not ready yet (Typst before typst-syntax.wasm loads); the caller draws no tokens.
export function tokenizeSource(language: Language, source: string): Token[] | null {
  return language.tokenize(source);
}

export function tokenizeOrEmpty(language: Language, source: string): Token[] {
  return tokenizeSource(language, source) ?? [];
}
