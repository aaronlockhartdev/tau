// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { md, mdHighlight, argsLines, valueLinesOf } from './markdown';

// The highlighter is the async shiki boundary; the markdown pass under test
// is how the base render and the substitution interact, so the test names
// tokens with a fake.
const { highlightHtml, esc } = vi.hoisted(() => {
  const esc = (s: string) =>
    s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  const highlightHtml = vi.fn((code: string, lang: string) =>
    Promise.resolve(`<i data-lang="${lang}">${code}</i>`)
  );
  return { highlightHtml, esc };
});
vi.mock('./highlight', () => ({ highlightHtml, esc }));

beforeEach(() => {
  highlightHtml.mockReset();
  highlightHtml.mockImplementation(
    (code: string, lang: string) => Promise.resolve(`<i data-lang="${lang}">${code}</i>`)
  );
});

describe('md', () => {
  it('renders plain text as a paragraph', () => {
    expect(md('hello')).toBe('<p>hello</p>\n');
  });

  it('renders a single newline as a break', () => {
    expect(md('a\nb')).toBe('<p>a<br>\nb</p>\n');
  });

  it('escapes raw html (bodies are model output, not trusted markup)', () => {
    const out = md('<script>alert(1)</script>');
    expect(out).not.toContain('<script>');
    expect(out).toContain('&lt;script&gt;');
  });

  it('renders a labeled fence as a plain escaped block with the language label', () => {
    const out = md('```rust\nlet x = 1;\n```');
    expect(out).toBe('<pre class="code">\n<span class="lang">rust</span>\nlet x = 1;\n</pre>\n');
  });

  it('renders a fence without a language (no lang span)', () => {
    const out = md('```\nplain\n```');
    expect(out).toBe('<pre class="code">plain\n</pre>\n');
  });

  it('escapes html inside a fence', () => {
    const out = md('```rust\nfn f() -> bool { true }\n```');
    expect(out).toContain('fn f() -&gt; bool { true }');
  });

  it('protects inline code from markdown interpretation', () => {
    const out = md('use `let x` carefully');
    expect(out).toContain('<code>let x</code>');
  });

  it('renders GFM tables', () => {
    const out = md('| a | b |\n| --- | --- |\n| 1 | 2 |');
    expect(out).toContain('<table>');
    expect(out).toContain('<th>a</th>');
    expect(out).toContain('<td>1</td>');
  });
});

describe('mdHighlight (#231)', () => {
  it('replaces a known-language fence with the highlighter output', async () => {
    const out = await mdHighlight('```rust\nlet x = 1;\n```');
    expect(out).toContain('<i data-lang="rust">let x = 1;</i>');
    expect(out).not.toContain('class="code"');
  });

  it('maps common fence label aliases to grammar names', async () => {
    await mdHighlight('```js\nconst a = 1;\n```');
    expect(highlightHtml).toHaveBeenCalledWith('const a = 1;', 'javascript');
  });

  it('passes an unknown label through (the highlighter degrades to plain)', async () => {
    const out = await mdHighlight('```notalanguage\nx\n```');
    expect(highlightHtml).toHaveBeenCalledWith('x', 'notalanguage');
    expect(out).toContain('<i data-lang="notalanguage">x</i>');
  });

  it('leaves an unlabeled fence plain', async () => {
    const out = await mdHighlight('```\nplain\n```');
    expect(highlightHtml).not.toHaveBeenCalled();
    expect(out).toBe('<pre class="code">plain\n</pre>\n');
  });

  it('highlights each labeled fence in a multi-fence body', async () => {
    const out = await mdHighlight('```rust\nfn a() {}\n```\n\ntext\n\n```json\n{"a": 1}\n```');
    expect(highlightHtml).toHaveBeenCalledTimes(2);
    expect(out).toContain('<i data-lang="rust">fn a() {}</i>');
    expect(out).toContain('<i data-lang="json">{"a": 1}</i>');
    expect(out).toContain('<p>text</p>');
  });

  it('keeps a plain block when the highlighter yields nothing (init failure)', async () => {
    highlightHtml.mockResolvedValueOnce('');
    const out = await mdHighlight('```rust\nlet x = 1;\n```');
    expect(out).toBe('<pre class="code">\n<span class="lang">rust</span>\nlet x = 1;\n</pre>\n');
  });

  it('leaves a body without fences untouched', async () => {
    const out = await mdHighlight('hello **world**');
    expect(highlightHtml).not.toHaveBeenCalled();
    expect(out).toBe('<p>hello <strong>world</strong></p>\n');
  });
});

describe('argsLines', () => {
  it('puts each field name on its own line with the value below', () => {
    expect(argsLines({ command: 'ls -la' })).toEqual([
      { k: 'command', lines: ['ls -la'] }
    ]);
  });

  it('indents nested objects one level per depth', () => {
    expect(argsLines({ a: { b: 'c' } })).toEqual([
      { k: 'a', lines: ['  • b: c'] }
    ]);
  });

  it('renders an array value as indented bullets', () => {
    expect(argsLines({ items: ['x', 'y'] })).toEqual([
      { k: 'items', lines: ['  • x', '  • y'] }
    ]);
  });

  it('renders a null array element as a dash bullet', () => {
    expect(argsLines({ items: [null, 1] })).toEqual([
      { k: 'items', lines: ['  • –', '  • 1'] }
    ]);
  });

  it('renders a null scalar as an empty value', () => {
    expect(argsLines({ a: null })).toEqual([{ k: 'a', lines: ['null'] }]);
  });

  it('folds an object array item leading with its text and the scalars as a tail', () => {
    expect(argsLines({ steps: [{ text: 'do it', status: 'pending' }] })).toEqual([
      { k: 'steps', lines: ['  • do it — status: pending'] }
    ]);
  });
});

describe('valueLinesOf', () => {
  it('stringifies a scalar', () => {
    expect(valueLinesOf(42, 1)).toEqual(['42']);
  });

  it('stringifies a null', () => {
    expect(valueLinesOf(null, 1)).toEqual(['null']);
  });

  it('renders a plain object as bullets', () => {
    expect(valueLinesOf({ a: 1, b: 2 }, 1)).toEqual(['  • a: 1', '  • b: 2']);
  });

  it('renders an array of objects item-wise (head key first)', () => {
    expect(valueLinesOf([{ id: 't1', title: 'x' }], 1)).toEqual(['  • t1 — title: x']);
  });

  it('renders an array item without a head key as plain object bullets', () => {
    expect(valueLinesOf([{ a: 1 }], 1)).toEqual(['  • a: 1']);
  });
});
