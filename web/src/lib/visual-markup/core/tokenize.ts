import { Language } from './language';
import { Tokenizer, TokenizeOptions } from './tokenizer';
import { Token } from './tokens';

// The tokens of a source. A language that defines tokenize() reads the shared
// syntax tree; the rule tokenizer is the fallback for the rest. maxDepth only
// applies to the fallback. Null means the language's tree is not ready yet.
export function tokenizeSource(language: Language, source: string, options: TokenizeOptions = {}): Token[] | null {
  if (language.tokenize) return language.tokenize(source);
  return new Tokenizer(language, options).tokenize(source);
}

export function tokenizeOrEmpty(language: Language, source: string): Token[] {
  return tokenizeSource(language, source) ?? [];
}
