import { describe, it, expect } from 'vitest';
import {
  assemble,
  buildDiffLines,
  editDiffCode,
  editDiffToDiffLines,
  langFor,
  parseEditDiff
} from './diffview';

describe('buildDiffLines', () => {
  it('interleaves context, added, and removed lines with new-file indices', () => {
    const base = 'a\nb\nc\nd';
    const next = 'a\nX\nc\nY'; // b→removed, X→added after a; d→removed, Y→added after c
    const d = buildDiffLines(base, next);
    expect(d).toEqual([
      { kind: 'context', text: 'a', newIdx: 0 },
      { kind: 'removed', text: 'b' },
      { kind: 'added', text: 'X', newIdx: 1 },
      { kind: 'context', text: 'c', newIdx: 2 },
      { kind: 'removed', text: 'd' },
      { kind: 'added', text: 'Y', newIdx: 3 },
    ]);
  });

  it('an untracked file (empty base) is all added, indexed from zero', () => {
    const d = buildDiffLines('', 'one\ntwo');
    expect(d).toEqual([
      { kind: 'added', text: 'one', newIdx: 0 },
      { kind: 'added', text: 'two', newIdx: 1 },
    ]);
  });

  it('a deleted file (empty next) is all removed', () => {
    const d = buildDiffLines('one\ntwo', '');
    expect(d).toEqual([{ kind: 'removed', text: 'one' }, { kind: 'removed', text: 'two' }]);
  });

  it('a trailing newline is not a line', () => {
    const d = buildDiffLines('a\n', 'a\nb\n');
    expect(d).toEqual([
      { kind: 'context', text: 'a', newIdx: 0 },
      { kind: 'added', text: 'b', newIdx: 1 },
    ]);
  });
});

describe('assemble', () => {
  it('numbers new-file lines, marks added, and leaves removed unnumbered', () => {
    const d = buildDiffLines('a\nb', 'a\nX');
    const tokens: { content: string; color?: string }[][] = [
      [{ content: 'a', color: '#fff' }],
      [{ content: 'X', color: '#0f0' }],
    ];
    const r = assemble(d, tokens);
    expect(r).toEqual([
      { key: 0, no: '1', mark: null, spans: [{ text: 'a', color: '#fff' }] },
      { key: 1, no: '', mark: 'del', spans: [{ text: 'b' }] },
      { key: 2, no: '2', mark: 'add', spans: [{ text: 'X', color: '#0f0' }] },
    ]);
  });

  it('a line with no tokens renders a single empty span', () => {
    const r = assemble([{ kind: 'context', text: 'x', newIdx: 0 }], [[]]);
    expect(r[0]?.spans).toEqual([]);
  });
});

describe('parseEditDiff', () => {
  it('parses the header, the removed/added/context body, and the truncation notice', () => {
    const text =
      'edited src/a.ts (lines 3–5 of 40)\n' +
      '-old one\n' +
      '+ abc new one\n' +
      ' ctx line\n' +
      '… 12 more lines changed';
    const d = parseEditDiff(text);
    expect(d?.header).toBe('edited src/a.ts (lines 3–5 of 40)');
    expect(d?.lines).toEqual([
      { kind: 'removed', text: 'old one' },
      { kind: 'added', text: 'new one', anchor: 'abc' },
      { kind: 'context', text: 'ctx line' }
    ]);
    expect(d?.truncated).toBe('… 12 more lines changed');
  });

  it('returns null only when there is no diff header', () => {
    // A no-change edit still carries a header (the GUI shows it, empty body).
    expect(parseEditDiff('edited src/a.ts (no change)')).toEqual({
      header: 'edited src/a.ts (no change)',
      lines: [],
      truncated: undefined
    });
    expect(parseEditDiff('some other output')).toBeNull();
  });

  it('treats an unknown leading char as context', () => {
    const d = parseEditDiff('edited a.ts (lines 1–1 of 2)\nX line');
    expect(d?.lines).toEqual([{ kind: 'context', text: 'X line' }]);
  });
});

describe('editDiffToDiffLines', () => {
  it('indexes added/context lines and leaves removed unnumbered', () => {
    const d = parseEditDiff('edited a.ts (lines 1–2 of 3)\n-old\n+ abc new\n ctx');
    expect(editDiffToDiffLines(d!)).toEqual([
      { kind: 'removed', text: 'old' },
      { kind: 'added', text: 'new', newIdx: 0 },
      { kind: 'context', text: 'ctx', newIdx: 1 }
    ]);
  });
});

describe('editDiffCode', () => {
  it('is the added+context fragment in order, without removed lines', () => {
    const d = parseEditDiff('edited a.ts (lines 1–2 of 3)\n-old\n+ abc new\n ctx');
    expect(editDiffCode(d!)).toBe('new\nctx');
  });
});

describe('langFor', () => {
  it('maps known extensions to a grammar', () => {
    expect(langFor('src/a.ts')).toBe('typescript');
    expect(langFor('a.tsx')).toBe('tsx');
    expect(langFor('crates/b.rs')).toBe('rust');
    expect(langFor('c.jsonc')).toBe('jsonc');
    expect(langFor('d.svelte')).toBe('svelte');
  });

  it('unknown extensions fall through to text', () => {
    expect(langFor('README')).toBe('text');
    expect(langFor('a.xyz')).toBe('text');
  });
});
