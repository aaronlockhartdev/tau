import type { SkillInfo } from './protocol';

export type Suggestion = {
  id: string;
  name: string;
  desc: string;
  skill?: SkillInfo;
};

// The command token being typed: the text after a leading `/`, up to the
// first space. Null when the input is not a slash command (no leading `/`,
// or the token was ended by a space).
export function slashToken(input: string): string | null {
  if (!input.startsWith('/')) return null;
  const rest = input.slice(1);
  return rest.includes(' ') ? null : rest;
}

// Case-insensitive prefix match of the token against each entry's name with
// the leading `/` stripped, so `/sk` matches every `/skill:*` entry.
export function filterSuggestions(all: Suggestion[], token: string): Suggestion[] {
  const q = token.toLowerCase();
  return all.filter((r) => r.name.slice(1).toLowerCase().startsWith(q));
}
