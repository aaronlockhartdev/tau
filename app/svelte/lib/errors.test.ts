import { describe, expect, it } from 'vitest';
import { errText } from './errors';

describe('errText', () => {
  it('returns the message of an Error', () => {
    expect(errText(new Error('boom'))).toBe('boom');
  });

  it('returns a plain string as-is', () => {
    expect(errText('plain')).toBe('plain');
  });

  it('returns the message of a serialized core error object', () => {
    expect(errText({ message: 'missing field `workspace`' })).toBe('missing field `workspace`');
  });

  it('falls back to the label when there is no message', () => {
    expect(errText({ label: 'not found' })).toBe('not found');
  });

  it('stringifies an object without message/label via JSON', () => {
    expect(errText({ code: 3 })).toBe('{"code":3}');
  });

  it('falls back to String() for a circular object (JSON.stringify throws)', () => {
    const o: Record<string, unknown> = { a: 1 };
    o.self = o;
    expect(errText(o)).toBe('[object Object]');
  });

  it('stringifies null, undefined and numbers', () => {
    expect(errText(null)).toBe('null');
    expect(errText(undefined)).toBe('undefined');
    expect(errText(42)).toBe('42');
  });
});
