// Tiny, dependency-free syntax highlighter for short code snippets.
//
// `tokenize` returns a flat list of `{ text, cls }` spans; the caller renders
// each as a plain Svelte text node (never innerHTML). It is deliberately
// approximate — good enough for the previews on the /code page — and
// truncation-tolerant: an unterminated string or comment in a partial chunk
// simply colors to the end of the snippet instead of throwing.

export type TokenClass =
  | 'tok-kw'
  | 'tok-fn'
  | 'tok-str'
  | 'tok-cmt'
  | 'tok-num'
  | 'tok-type'
  | '';

export interface Token {
  text: string;
  cls: TokenClass;
}

const COMMON = [
  'return', 'if', 'else', 'for', 'while', 'break', 'continue', 'switch', 'case',
  'default', 'true', 'false', 'null', 'new', 'class', 'import', 'export', 'try',
  'catch', 'finally', 'throw', 'this', 'void',
];

const KEYWORDS: Record<string, string[]> = {
  rust: [
    'fn', 'let', 'mut', 'const', 'static', 'struct', 'enum', 'trait', 'impl',
    'pub', 'use', 'mod', 'match', 'if', 'else', 'for', 'while', 'loop', 'return',
    'self', 'Self', 'crate', 'super', 'where', 'as', 'dyn', 'ref', 'move', 'async',
    'await', 'unsafe', 'in', 'type', 'continue', 'break', 'box', 'true', 'false',
  ],
  python: [
    'def', 'class', 'return', 'if', 'elif', 'else', 'for', 'while', 'import',
    'from', 'as', 'with', 'try', 'except', 'finally', 'raise', 'yield', 'lambda',
    'pass', 'break', 'continue', 'global', 'nonlocal', 'in', 'is', 'not', 'and',
    'or', 'None', 'True', 'False', 'async', 'await', 'del', 'assert',
  ],
  javascript: [
    ...COMMON, 'function', 'const', 'let', 'var', 'typeof', 'instanceof', 'of',
    'async', 'await', 'yield', 'from', 'extends', 'super', 'delete', 'undefined',
  ],
  typescript: [
    ...COMMON, 'function', 'const', 'let', 'var', 'typeof', 'instanceof', 'of',
    'async', 'await', 'yield', 'from', 'extends', 'super', 'delete', 'undefined',
    'interface', 'type', 'enum', 'implements', 'public', 'private', 'protected',
    'readonly', 'keyof', 'namespace', 'declare', 'as', 'abstract',
  ],
  go: [
    'func', 'var', 'const', 'type', 'struct', 'interface', 'map', 'chan',
    'package', 'import', 'return', 'if', 'else', 'for', 'range', 'switch', 'case',
    'default', 'go', 'defer', 'select', 'break', 'continue', 'fallthrough', 'nil',
    'true', 'false', 'make', 'new',
  ],
  java: [
    ...COMMON, 'interface', 'enum', 'extends', 'implements', 'public', 'private',
    'protected', 'static', 'final', 'abstract', 'synchronized', 'package', 'throws',
  ],
  c: [
    'int', 'char', 'float', 'double', 'void', 'short', 'long', 'unsigned', 'signed',
    'struct', 'enum', 'union', 'typedef', 'static', 'const', 'return', 'if', 'else',
    'for', 'while', 'switch', 'case', 'break', 'continue', 'sizeof', 'goto', 'do',
    'default', 'extern', 'volatile', 'register',
  ],
  cpp: [
    'int', 'char', 'float', 'double', 'void', 'bool', 'auto', 'short', 'long',
    'unsigned', 'signed', 'struct', 'enum', 'union', 'typedef', 'static', 'const',
    'constexpr', 'return', 'if', 'else', 'for', 'while', 'switch', 'case', 'break',
    'continue', 'sizeof', 'class', 'public', 'private', 'protected', 'virtual',
    'override', 'namespace', 'template', 'typename', 'using', 'new', 'delete',
    'this', 'nullptr', 'true', 'false', 'friend', 'operator', 'try', 'catch', 'throw',
  ],
};

function keywordsFor(language: string): Set<string> {
  const lang = (language || '').toLowerCase();
  const key = lang === 'tsx' ? 'typescript' : lang === 'jsx' ? 'javascript' : lang;
  return new Set(KEYWORDS[key] ?? []);
}

const isIdentStart = (c: string) => /[A-Za-z_$]/.test(c);
const isIdent = (c: string) => /[A-Za-z0-9_$]/.test(c);
const isDigit = (c: string) => c >= '0' && c <= '9';

export function tokenize(code: string, language: string): Token[] {
  const kw = keywordsFor(language);
  const hashComment = (language || '').toLowerCase() === 'python';
  const out: Token[] = [];
  const n = code.length;
  let i = 0;
  let plain = '';
  const flush = () => {
    if (plain) {
      out.push({ text: plain, cls: '' });
      plain = '';
    }
  };
  const push = (text: string, cls: TokenClass) => {
    flush();
    out.push({ text, cls });
  };

  while (i < n) {
    const c = code[i];
    const next = i + 1 < n ? code[i + 1] : '';

    // line comment
    if ((c === '/' && next === '/') || (hashComment && c === '#')) {
      let j = i;
      while (j < n && code[j] !== '\n') j++;
      push(code.slice(i, j), 'tok-cmt');
      i = j;
      continue;
    }
    // block comment (truncation-tolerant)
    if (c === '/' && next === '*') {
      let j = i + 2;
      while (j < n && !(code[j] === '*' && code[j + 1] === '/')) j++;
      j = j < n ? j + 2 : n;
      push(code.slice(i, j), 'tok-cmt');
      i = j;
      continue;
    }
    // string (single/double/backtick, escape-aware, truncation-tolerant)
    if (c === '"' || c === "'" || c === '`') {
      let j = i + 1;
      while (j < n && code[j] !== c) {
        if (code[j] === '\\') j++;
        j++;
      }
      j = j < n ? j + 1 : n;
      push(code.slice(i, j), 'tok-str');
      i = j;
      continue;
    }
    // number
    if (isDigit(c)) {
      let j = i;
      while (j < n && /[0-9a-fA-FxXoObB._]/.test(code[j])) j++;
      push(code.slice(i, j), 'tok-num');
      i = j;
      continue;
    }
    // identifier / keyword / type / function-call
    if (isIdentStart(c)) {
      let j = i;
      while (j < n && isIdent(code[j])) j++;
      const word = code.slice(i, j);
      let k = j;
      while (k < n && (code[k] === ' ' || code[k] === '\t')) k++;
      const callsLike = code[k] === '(';
      let cls: TokenClass;
      if (kw.has(word)) cls = 'tok-kw';
      else if (callsLike) cls = 'tok-fn';
      else if (/^[A-Z]/.test(word)) cls = 'tok-type';
      else cls = '';
      if (cls) push(word, cls);
      else plain += word;
      i = j;
      continue;
    }
    plain += c;
    i++;
  }
  flush();
  return out;
}
