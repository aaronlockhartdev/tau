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
  // jsdiff's line diff is sensitive to a missing final newline: it can diff the
  // last line as changed when one leg ends in \n and the other doesn't. Normalize
  // both legs to end in a newline so a trailing-newline-only difference is a no-op
  // (an empty leg stays empty).
  const norm = (s: string) => (s && !s.endsWith('\n') ? s + '\n' : s);
  const b = norm(base);
  const n = norm(next);
  for (const c of diffLines(b, n)) {
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

// The edit tool's bounded-diff output (the contract the core emits): a summary
// header, then body lines whose FIRST char is the marker — `-` removed, `+`
// added (carrying the fresh 3-char anchor), ` ` context — and an optional
// truncation notice. Parsed so the card renders it as a colored git-style diff.
export interface EditDiffLine {
  kind: 'removed' | 'added' | 'context';
  text: string;
  anchor?: string;
}
export interface EditDiff {
  header: string;
  lines: EditDiffLine[];
  truncated?: string;
}

export function parseEditDiff(text: string): EditDiff | null {
  const raw = text.split('\n');
  const header = raw[0] ?? '';
  if (!header.startsWith('edited ')) return null;
  const lines: EditDiffLine[] = [];
  let truncated: string | undefined;
  for (const row of raw.slice(1)) {
    if (row === '') continue;
    const m = row[0];
    if (m === '…') {
      truncated = row;
    } else if (m === '-') {
      lines.push({ kind: 'removed', text: row.slice(1) });
    } else if (m === '+') {
      // "+ {hash} {line}": a space after the +, the fresh 3-char anchor, a
      // space, then the added line (matches the core's emitter exactly).
      lines.push({ kind: 'added', text: row.slice(6), anchor: row.slice(2, 5) });
    } else if (m === ' ') {
      lines.push({ kind: 'context', text: row.slice(1) });
    } else {
      lines.push({ kind: 'context', text: row });
    }
  }
  return { header, lines, truncated };
}

// Map a parsed edit diff onto the file-view diff model: added/context lines
// carry a running new-file index (so assemble pulls their syntax tokens);
// removed lines are unnumbered ghosts.
export function editDiffToDiffLines(d: EditDiff): DiffLine[] {
  let newIdx = 0;
  return d.lines.map((l) =>
    l.kind === 'removed'
      ? { kind: 'removed', text: l.text }
      : { kind: l.kind, text: l.text, newIdx: newIdx++ }
  );
}

// The new-file fragment (added + context lines, in order) as one string — the
// input to highlightLines for an edit diff.
export function editDiffCode(d: EditDiff): string {
  return d.lines.filter((l) => l.kind !== 'removed').map((l) => l.text).join('\n');
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
