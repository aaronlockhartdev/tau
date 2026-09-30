import { describe, expect, it } from 'vitest';
import { md, argsLines, valueLinesOf } from './markdown';

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

  it('renders a fenced code block through the highlighter, with the language label', () => {
    const out = md('```rust\nlet x = 1;\n```');
    expect(out).toContain('<pre class="code">');
    expect(out).toContain('<span class="lang">rust</span>');
    expect(out).toContain('<span class="k">let</span>');
    expect(out).toContain('<span class="n">1</span>');
  });

  it('renders a fence without a language (no lang span)', () => {
    const out = md('```\nplain\n```');
    expect(out).toContain('<pre class="code">');
    expect(out).not.toContain('class="lang"');
  });

  it('protects inline code from markdown interpretation', () => {
    const out = md('use `let x` carefully');
    expect(out).toContain('<code>let x</code>');
    expect(out).not.toContain('<span class="k">let</span>');
  });

  it('highlights comments, strings, capitalized identifiers and numbers', () => {
    const out = md('```c\n// note "str"\nValue 42\n```');
    expect(out).toContain('<span class="c">// note "str"</span>');
    expect(out).toContain('<span class="t2">Value</span>');
    expect(out).toContain('<span class="n">42</span>');
  });

  it('renders GFM tables', () => {
    const out = md('| a | b |\n| --- | --- |\n| 1 | 2 |');
    expect(out).toContain('<table>');
    expect(out).toContain('<th>a</th>');
    expect(out).toContain('<td>1</td>');
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
