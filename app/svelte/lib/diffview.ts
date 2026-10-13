import { diffLines } from 'diff';

// The file-content view's render model. A line is one of the new file's
// lines (context or added, carrying its new-file index) or a ghost deletion
// (removed — present in the base, gone from the file).
export type DiffLine =
  | { kind: 'context' | 'added'; text: string; newIdx: number }
  | { kind: 'removed'; text: string };

export type Mark = 'add' | 'del' | null;
export type Span = { text: string; color?: string };
export type RenderLine = { key: number; no: string; mark: Mark; spans: Span[] };

// Walk a two-line diff (jsdiff) into the interleaved file order: new-file
// lines keep their index (so the caller can pull syntax tokens by line);
// deleted lines become ghosts. A trailing newline yields a final empty split
// element, which is not a line and is dropped.
export function buildDiffLines(base: string, next: string): DiffLine[] {
  const out: DiffLine[] = [];
  let newIdx = 0;
  for (const c of diffLines(base, next)) {
    const lines = c.value.split('\n');
    if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();
    for (const text of lines) {
      if (c.added) out.push({ kind: 'added', text, newIdx: newIdx++ });
      else if (c.removed) out.push({ kind: 'removed', text });
      else out.push({ kind: 'context', text, newIdx: newIdx++ });
    }
  }
  return out;
}

// Map the diff sequence onto the new file's per-line syntax tokens:
// context/added lines pull their line's tokens by new-file index; removed
// lines are uncolored ghosts (the text no longer exists in the file).
type Token = { content: string; color?: string };
export function assemble(diff: DiffLine[], tokens: Token[][]): RenderLine[] {
  let key = 0;
  return diff.map((d) => {
    if (d.kind === 'removed') {
      return { key: key++, no: '', mark: 'del' as Mark, spans: [{ text: d.text }] };
    }
    const toks = tokens[d.newIdx] ?? [];
    return {
      key: key++,
      no: String(d.newIdx + 1),
      mark: d.kind === 'added' ? 'add' : null,
      spans: toks.map((t) => ({ text: t.content, color: t.color })),
    };
  });
}

// Extension → shiki language id; unknown extensions fall through to 'text'
// (rendered plain, no grammar).
const BY_EXT: Record<string, string> = {
  ts: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  js: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  tsx: 'tsx',
  jsx: 'jsx',
  rs: 'rust',
  json: 'json',
  jsonc: 'jsonc',
  md: 'markdown',
  svelte: 'svelte',
  toml: 'toml',
  yml: 'yaml',
  yaml: 'yaml',
  css: 'css',
  html: 'html',
  sh: 'bash',
};

export function langFor(path: string): string {
  const name = path.split('/').pop() ?? path;
  const ext = name.includes('.') ? (name.split('.').pop() ?? '').toLowerCase() : '';
  return BY_EXT[ext] ?? 'text';
}
