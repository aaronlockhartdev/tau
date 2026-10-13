import { createHighlighter, type BundledLanguage, type Highlighter } from 'shiki';

// dark-plus is VS Code's default dark theme: the file-content view matches
// the editor look the rest of the workbench aims at.
const THEME = 'dark-plus';
// The grammars the workbench opens. The full bundle ships every language;
// naming a subset only trims the highlighter's startup cost.
const LANGS = [
  'typescript',
  'javascript',
  'tsx',
  'jsx',
  'rust',
  'json',
  'jsonc',
  'markdown',
  'toml',
  'bash',
  'css',
  'svelte',
  'html',
  'yaml',
] as const;

type Token = { content: string; color?: string };
let instance: Promise<Highlighter> | null = null;

function hl(): Promise<Highlighter> {
  if (instance === null) instance = createHighlighter({ themes: [THEME], langs: [...LANGS] });
  return instance;
}

// Per-line syntax tokens for a file body. A real grammar yields shiki's
// colored tokens; plain text (or a grammar not in LANGS) yields one
// uncolored token per line so the caller renders the default foreground.
export async function highlightLines(code: string, lang: string): Promise<Token[][]> {
  if (lang === 'text' || !(LANGS as readonly string[]).includes(lang)) {
    return code.split('\n').map((line) => [{ content: line }]);
  }
  const h = await hl();
  // codeToTokens is synchronous once the highlighter is initialized; only
  // createHighlighter (above) is async.
  return h.codeToTokens(code, { lang: lang as BundledLanguage, theme: THEME }).tokens;
}
