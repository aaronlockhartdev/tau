import { describe, expect, it } from 'vitest';
import type { Entry } from './protocol';
import { streamClock } from './stream.svelte';

// The stream clock (#303): a delta applies only as the next frame of its
// entry's run, and the entry's file line retires the run. The clock is keyed
// session → entry id, because ids are minted per session and repeat.
describe('streamClock (ADR-0008, #303)', () => {
  function ev(session: string, id: string, seq: number, text: string) {
    return { session, id, seq, text, reasoning: '' };
  }

  function committed(entries: Record<string, Entry>, id: string, text: string): void {
    entries[id] = { id, kind: 'message', text };
  }

  function text(entries: Record<string, Entry>, id: string): string {
    const card = entries[id];
    return card && card.kind === 'message' ? (card.text ?? '') : '';
  }

  it('applies a run frame by frame in order', () => {
    const entries: Record<string, Entry> = {};
    expect(streamClock.apply(entries, ev('clock-order', 'a1', 1, 'one'))).toBe(true);
    expect(streamClock.apply(entries, ev('clock-order', 'a1', 2, ' two'))).toBe(true);
    expect(text(entries, 'a1')).toBe('one two');
  });

  // The wiring behind entry_upsert: the record replaces the card, so a frame
  // that arrives after it must add nothing — a committed card never grows.
  it('a frame after the record landed appends nothing to the committed card', () => {
    const entries: Record<string, Entry> = {};
    expect(streamClock.apply(entries, ev('clock-settle', 'a1', 1, 'a'))).toBe(true);
    committed(entries, 'a1', 'the record’s full text');
    streamClock.settle('clock-settle', 'a1');
    expect(streamClock.apply(entries, ev('clock-settle', 'a1', 2, 'd'))).toBe(false);
    expect(text(entries, 'a1')).toBe('the record’s full text');
  });

  it('the same entry id in another session runs its own sequence', () => {
    const entries: Record<string, Entry> = {};
    expect(streamClock.apply(entries, ev('clock-s1', 'a1', 1, 'one'))).toBe(true);
    expect(streamClock.apply(entries, ev('clock-s2', 'a1', 1, 'two'))).toBe(true);
    expect(text(entries, 'a1')).toBe('onetwo');
  });

  it('a new call’s clear restarts the sequence for a re-minted id', () => {
    const entries: Record<string, Entry> = {};
    expect(streamClock.apply(entries, ev('clock-clear', 'a1', 1, 'one'))).toBe(true);
    streamClock.clearStream('clock-clear');
    delete entries['a1'];
    expect(streamClock.apply(entries, ev('clock-clear', 'a1', 1, 'fresh'))).toBe(true);
    expect(text(entries, 'a1')).toBe('fresh');
  });
});
