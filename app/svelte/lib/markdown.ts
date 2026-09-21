// Markdown + syntax highlighting, ported from the prototype's renderer
// (prototype/gui-ia/index.html): code segments are PROTECTED from markdown
// interpretation — fences are split out first and only non-code text gets
// inline rules, with inline code placeholdered so it is never re-parsed.

function esc(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

// The prototype's highlighter: comments, strings, a keyword set, numbers,
// capitalized identifiers. A small highlighter per the ticket's allowance
// (zero-dependency, matching the prototype's behavior).
export function hl(code: string): string {
  let s = esc(code);
  s = s.replace(/(\/\/[^\n]*|#[^\n]*)/g, '<span class="c">$1</span>');
  s = s.replace(/(&quot;[^&]*?&quot;|&apos;[^&]*?&apos;|'[^'\n]*')/g, '<span class="s">$1</span>');
  const kw = ['let', 'fn', 'pub', 'struct', 'enum', 'impl', 'match', 'use', 'return', 'if', 'else', 'while', 'for', 'const', 'mod', 'move', 'mut', 'crate', 'self', 'Some', 'None', 'Ok'];
  for (const k of kw) {
    s = s.replace(new RegExp('(\\b' + k + '\\b)(?![^<]*</span>)', 'g'), '<span class="k">$1</span>');
  }
  s = s.replace(/\b(\d[\d_]*)\b/g, '<span class="n">$1</span>');
  s = s.replace(/\b([A-Z][A-Za-z0-9]*)\b/g, '<span class="t2">$1</span>');
  return s;
}

interface Seg {
  code?: string;
  lang?: string;
  txt?: string;
}

export function md(t: string): string {
  // Fences are split from the RAW text: text segments are escaped here, code
  // segments are escaped exactly once by hl() — escaping the fence text up
  // front and again in hl() double-encodes (& becomes &amp;amp;).
  const segs: Seg[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  const fenceRe = /```(\w*)\n([\s\S]*?)```/g;
  while ((m = fenceRe.exec(t))) {
    if (m.index > 0) segs.push({ txt: esc(t.slice(last, m.index)) });
    segs.push({ code: m[2], lang: m[1] });
    last = m.index + m[0].length;
  }
  if (last < t.length) segs.push({ txt: esc(t.slice(last)) });

  let out = '';
  for (const sg of segs) {
    if (sg.code != null) {
      out +=
        '<pre class="code">' +
        (sg.lang ? '<span class="lang">' + sg.lang + '</span>' : '') +
        hl(sg.code) +
        '</pre>';
    } else {
      let x = sg.txt ?? '';
      const codes: string[] = [];
      x = x.replace(/`([^`\n]+)`/g, (mm) => {
        codes.push(mm.slice(1, -1));
        return '\u0000' + (codes.length - 1) + '\u0000';
      });
      x = x.replace(/\*\*([^*]+)\*\*/g, '<b>$1</b>').replace(/\*([^*\n]+)\*/g, '<i>$1</i>');
      x = x.replace(/^### (.*)$/gm, '<div class="mh">$1</div>').replace(/^## (.*)$/gm, '<div class="mh2">$1</div>');
      x = x.replace(/^- (.*)$/gm, '<div class="mi">$1</div>');
      x = x.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<span class="lk">$1</span>');
      x = x.replace(/\u0000(\d+)\u0000/g, (_mm, i2) => '<code class="ic">' + codes[+i2] + '</code>');
      out += x;
    }
  }
  return out;
}

export { esc };

// A sub-agent's message to the parent (and a parent's message to a child)
// conventionally carries a JSON payload after the prose ("hi — {"word":"hi"}").
// Split it out so the GUI can render the payload as kv lines, the way the
// expanded tool call does. Returns null when the text has no parseable
// trailing object.
export function splitJsonPayload(
  text: string
): { prose: string; kv: [string, string][] } | null {
  const t = text.trimEnd();
  if (!t.endsWith('}')) return null;
  for (let i = t.length - 1; i >= 0; i--) {
    if (t[i] !== '{') continue;
    let obj: unknown;
    try {
      obj = JSON.parse(t.slice(i));
    } catch {
      continue;
    }
    if (typeof obj !== 'object' || obj === null || Array.isArray(obj)) continue;
    const kv = Object.entries(obj).map(
      ([k, v]) => [k, typeof v === 'string' ? v : JSON.stringify(v)] as [string, string]
    );
    if (!kv.length) continue;
    return { prose: t.slice(0, i).replace(/[\s—–\-·:]+$/, ''), kv };
  }
  return null;
}
