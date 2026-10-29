import { describe, expect, it } from 'vitest';
import { highlightLines } from './highlight';

describe('highlightLines', () => {
  it('highlights a language outside the old 14-subset (php)', async () => {
    const lines = await highlightLines('<?php echo 1; ?>', 'php');
    expect(lines.length).toBe(1);
    // A real grammar yields at least one colored token.
    expect(lines[0]?.some((t) => t.color)).toBe(true);
  });

  it('returns plain uncolored tokens for text', async () => {
    const lines = await highlightLines('hello\nworld', 'text');
    expect(lines).toEqual([
      [{ content: 'hello' }],
      [{ content: 'world' }],
    ]);
  });

  it('returns plain uncolored tokens for a language shiki does not bundle', async () => {
    const lines = await highlightLines('hello', 'definitely-not-a-lang');
    expect(lines).toEqual([[{ content: 'hello' }]]);
  });
});
