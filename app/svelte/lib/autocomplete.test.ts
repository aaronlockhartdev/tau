// The composer's slash-command trigger + filter (pure; the dropdown's data
// source is the workspace skill registry in the store).
import { describe, expect, it } from 'vitest';
import { filterSuggestions, slashToken, type Suggestion } from './autocomplete';

const ALL: Suggestion[] = [
  { id: 'model', name: '/model', desc: "switch the session's model" },
  { id: 'help', name: '/help', desc: 'list commands' },
  { id: 'skill:alpha', name: '/skill:alpha', desc: 'a' },
  { id: 'skill:bravo', name: '/skill:bravo', desc: 'b' }
];

const names = (rows: Suggestion[]) => rows.map((r) => r.name);

describe('slashToken', () => {
  it('is null for non-slash input', () => {
    expect(slashToken('')).toBeNull();
    expect(slashToken('hello')).toBeNull();
    expect(slashToken('hello /sk')).toBeNull();
  });

  it('is empty for a bare /', () => {
    expect(slashToken('/')).toBe('');
  });

  it('is the text after / up to the first space', () => {
    expect(slashToken('/sk')).toBe('sk');
    expect(slashToken('/skill:al')).toBe('skill:al');
  });

  it('is null once a space ends the token', () => {
    expect(slashToken('/model args')).toBeNull();
    expect(slashToken('/ ')).toBeNull();
  });
});

describe('filterSuggestions', () => {
  it('lists every entry for an empty token', () => {
    expect(names(filterSuggestions(ALL, ''))).toEqual([
      '/model',
      '/help',
      '/skill:alpha',
      '/skill:bravo'
    ]);
  });

  it('narrows to entries whose name prefix-matches the token, case-insensitively', () => {
    expect(names(filterSuggestions(ALL, 'sk'))).toEqual(['/skill:alpha', '/skill:bravo']);
    expect(names(filterSuggestions(ALL, 'model'))).toEqual(['/model']);
    expect(names(filterSuggestions(ALL, 'HELP'))).toEqual(['/help']);
    expect(names(filterSuggestions(ALL, 'skill:al'))).toEqual(['/skill:alpha']);
    expect(names(filterSuggestions(ALL, 'zzz'))).toEqual([]);
  });
});
