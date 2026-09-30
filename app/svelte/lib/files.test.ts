import { describe, expect, it } from 'vitest';
import {
  ensureWorkspace,
  putListing,
  removeWorkspace,
  scheduleRefetch,
  toggleDir,
  waveDirs,
  type FilesCache
} from './files';
import type { FileEntry } from './protocol';

const f: FileEntry = { name: 'x', path: 'x', dir: false, size: 1 };
const d: FileEntry = { name: 'src', path: 'src', dir: true, size: 0 };

describe('ensureWorkspace / removeWorkspace', () => {
  it('ensure creates a missing workspace, keeps an existing cache reference', () => {
    const c: FilesCache = { w1: {} };
    expect(ensureWorkspace(c, 'w1')).toBe(c);
    expect(ensureWorkspace(c, 'w2')).toEqual({ w1: {}, w2: {} });
  });

  it('remove drops the workspace, keeps the cache for an unknown one', () => {
    const c: FilesCache = { w1: { '.': [f] }, w2: {} };
    expect(removeWorkspace(c, 'w1')).toEqual({ w2: {} });
    expect(removeWorkspace(c, 'w9')).toBe(c);
  });
});

describe('putListing', () => {
  it('is a no-op (same reference) for an unchanged listing', () => {
    const c: FilesCache = { w1: { '.': [f] } };
    expect(putListing(c, 'w1', '.', [f])).toBe(c);
  });

  it('replaces a changed listing', () => {
    const c: FilesCache = { w1: { '.': [f] } };
    expect(putListing(c, 'w1', '.', [f, d])).toEqual({ w1: { '.': [f, d] } });
  });

  it('is a no-op for an uncreated workspace', () => {
    const c: FilesCache = {};
    expect(putListing(c, 'w1', '.', [f])).toBe(c);
  });
});

describe('toggleDir', () => {
  it('is a no-op for an uncreated workspace', () => {
    const c: FilesCache = {};
    expect(toggleDir(c, 'w1', 'src')).toEqual({ cache: c, fetch: null });
  });

  it('expanding an unlisted dir lists it empty and names the fetch', () => {
    const c: FilesCache = { w1: {} };
    expect(toggleDir(c, 'w1', 'src')).toEqual({ cache: { w1: { src: [] } }, fetch: 'src' });
  });

  it('collapsing a listed dir drops the listing, no fetch', () => {
    const c: FilesCache = { w1: { src: [f] } };
    expect(toggleDir(c, 'w1', 'src')).toEqual({ cache: { w1: {} }, fetch: null });
  });
});

describe('scheduleRefetch / waveDirs', () => {
  it('schedules only listed dirs (a change in an unlisted dir waits for expansion)', () => {
    const c: FilesCache = { w1: { '.': [f] } };
    const pending = new Map<string, Set<string>>();
    scheduleRefetch(c, pending, 'w1', ['.', 'src']);
    expect(waveDirs(c, pending)).toEqual([{ ws: 'w1', path: '.' }]);
  });

  it('is a no-op for an uncreated workspace', () => {
    const pending = new Map<string, Set<string>>();
    scheduleRefetch({}, pending, 'w1', ['.']);
    expect(pending.size).toBe(0);
  });

  it('a dir collapsed inside the window drops out of the wave', () => {
    const c: FilesCache = { w1: {} };
    const pending = new Map<string, Set<string>>([['w1', new Set(['src'])]]);
    expect(waveDirs(c, pending)).toEqual([]);
    expect(pending.has('w1')).toBe(false);
  });

  it('drops a wave for a workspace that closed mid-flight', () => {
    const c: FilesCache = {};
    const pending = new Map<string, Set<string>>([['w1', new Set(['src'])]]);
    expect(waveDirs(c, pending)).toEqual([]);
  });

  it('coalesces repeated schedules into one wave entry', () => {
    const c: FilesCache = { w1: { '.': [f], src: [f] } };
    const pending = new Map<string, Set<string>>();
    scheduleRefetch(c, pending, 'w1', ['.']);
    scheduleRefetch(c, pending, 'w1', ['src', '.']);
    expect(waveDirs(c, pending).sort((a, b) => a.path.localeCompare(b.path))).toEqual([
      { ws: 'w1', path: '.' },
      { ws: 'w1', path: 'src' }
    ]);
  });
});
