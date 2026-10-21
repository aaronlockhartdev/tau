import MarkdownIt from 'markdown-it';
import { highlightHtml } from './highlight';

// Markdown rendering, ported from the prototype's renderer: code segments
// are PROTECTED from markdown interpretation — fences are split out first
// and only non-code text gets inline rules, with inline code
// placeholdered so it is never re-parsed.

function esc(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

// A fenced code block: an escaped plain block in the app's .code pre
// (markdownit's highlight hook returns the block verbatim). mdHighlight
// replaces the block with shiki's colored spans after the render.
function highlight(code: string, lang: string): string {
  return (
    '<pre class="code">' +
    (lang ? '\n<span class="lang">' + lang + '</span>\n' : '') +
    esc(code) +
    '</pre>'
  );
}

// CommonMark + GFM tables; single newlines render as <br> (breaks). Raw HTML
// is off (bodies are model output, not trusted markup).
const mdit = new MarkdownIt({ html: false, breaks: true, highlight });
mdit.enable(['table']);

// The synchronous base render: every fenced block is plain (escaped) text.
export function md(t: string): string {
  return mdit.render(t);
}

// Fences the base render leaves in plain text: the .code pre with a language
// label above the escaped code. $1 = label, $2 = escaped code.
const PLAIN_FENCE = /<pre class="code">\n<span class="lang">([^<]+)<\/span>\n([\s\S]*?)<\/pre>/g;

// Fence labels models emit, mapped onto the shiki grammar names the file
// view already uses. A label outside this table (or an unlabeled fence) is
// not guessed: it stays plain text. (#231)
const LANG_ALIAS: Record<string, string> = {
  js: 'javascript',
  ts: 'typescript',
  py: 'python',
  rb: 'ruby',
  yml: 'yaml',
  sh: 'bash',
  shell: 'bash',
  zsh: 'bash',
  'c++': 'cpp',
  'c#': 'csharp',
  golang: 'go'
};

// The async pass: every fenced block whose label names a known grammar gets
// shiki's colored spans; unlabeled, unknown-language, and init-failure cases
// keep the base's plain block. Highlighting is cosmetic — it must never
// change what a fence says.
export async function mdHighlight(t: string): Promise<string> {
  const base = md(t);
  const jobs: Array<{ block: string; lang: string; code: string }> = [];
  const scanned = base.replace(PLAIN_FENCE, (block: string, lang: string, code: string) => {
    jobs.push({ block, lang, code: unescapeHtml(code) });
    return block;
  });
  const replacements: string[] = Array.from({ length: jobs.length }, () => '');
  await Promise.all(
    jobs.map(async (j, i) => {
      // The fence's trailing newline belongs to the block, not the code.
      const code = j.code.replace(/\n$/, '');
      const out = await highlightHtml(code, LANG_ALIAS[j.lang] ?? j.lang);
      // Empty = plain text (init failure): keep the base's plain block.
      replacements[i] = out ? out + '\n' : j.block;
    })
  );
  let k = -1;
  return scanned.replace(PLAIN_FENCE, () => replacements[++k] ?? '');
}

function unescapeHtml(s: string): string {
  return s.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&');
}

// Structured lines for a tool's arguments: each field's name on its own
// line with a colon, the value on the lines below (scalars a single line,
// objects and arrays as indented bullets), so nothing renders as a raw
// JSON blob.
export function argsLines(a: Record<string, unknown>): Array<{ k: string; lines: string[] }> {
  return Object.entries(a).map(([k, v]) => ({ k, lines: valueLinesOf(v, 1) }));
}

export function valueLinesOf(v: unknown, depth: number): string[] {
  if (Array.isArray(v)) {
    return v.flatMap((x) =>
      x === null
        ? [bullet(depth, '–')]
        : typeof x === 'object'
          ? itemLines(x as Record<string, unknown>, depth)
          : [bullet(depth, String(x))]
    );
  }
  if (typeof v === 'object' && v !== null) return objLines(v as Record<string, unknown>, depth);
  return [String(v)];
}

function scalarText(v: unknown): string {
  return typeof v === 'string' ? v : String(v);
}

function objLines(o: Record<string, unknown>, depth: number): string[] {
  const out: string[] = [];
  for (const [k, v] of Object.entries(o)) {
    if (Array.isArray(v) || (typeof v === 'object' && v !== null)) {
      out.push(bullet(depth, `${k}:`));
      out.push(...valueLinesOf(v, depth + 1));
    } else {
      out.push(bullet(depth, `${k}: ${v === null ? '' : scalarText(v)}`));
    }
  }
  return out;
}

// A bullet line; an object item leads with its text/summary/id when it
// has one and folds its remaining scalars onto the same line.
function itemLines(o: Record<string, unknown>, depth: number): string[] {
  const head = o.text ?? o.summary ?? o.id ?? o.command;
  if (typeof head === 'string') {
    const headKey = (['text', 'summary', 'id', 'command'] as const).find(
      (k) => o[k] === head
    );
    const rest = Object.entries(o).filter(([k, v]) => k !== headKey && v !== null);
    const tail = rest
      .filter(([, v]) => typeof v !== 'object')
      .map(([k, v]) => `${k}: ${scalarText(v)}`)
      .join(', ');
    return [bullet(depth, tail ? `${head} — ${tail}` : head)];
  }
  return objLines(o, depth);
}

function bullet(depth: number, text: string): string {
  return '  '.repeat(depth) + '• ' + text;
}
