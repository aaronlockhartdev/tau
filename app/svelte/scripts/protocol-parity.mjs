#!/usr/bin/env node
// ADR-0006's TS mirror (app/svelte/lib/protocol.ts) must not silently drift
// from the Rust protocol surface (crates/tau-protocol): the guard diffs
// every enum/union (tags + fields) and struct (fields) field-for-field,
// extended to the C9 payload shapes when the crate carries them.
//
// Parity allowances, each a deliberate narrowing direction:
// - Rust `Value` (and `Vec<Value>`) is the wire's free-form floor (ADR-0005);
//   it is parity with any TS type — the C9 typed layer sits on top of it.
// - Rust `String` is parity with a TS union of string literals (the mirror
//   names the known values; the wire stays a string).
// - A Rust struct referenced by name is parity with a TS inline object of
//   the same normalized fields (CommandOutput.file mirrors FileText inline).
// - `#[serde(default)]` makes a field wire-optional; the TS mirror may carry
//   it required (the writer always fills it).
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const rustSrc = [
  readFileSync(join(root, 'crates/tau-protocol/src/lib.rs'), 'utf8'),
  readFileSync(join(root, 'crates/tau-protocol/src/snapshot.rs'), 'utf8'),
  readFileSync(join(root, 'crates/tau-protocol/src/payload.rs'), 'utf8'),
  readFileSync(join(root, 'crates/tau-protocol/src/coalesce.rs'), 'utf8')
].join('\n');
const tsSrc = readFileSync(join(root, 'app/svelte/lib/protocol.ts'), 'utf8');

const errors = [];
const infos = [];
// The crate names a few types the TS mirror renames (the GUI's vocabulary).
const ALIASES = { AgentType: 'AgentInfo' };

function snake(name) {
  return name.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toLowerCase();
}
function stripComments(src) {
  return src
    .split('\n')
    .filter((l) => !l.trimStart().startsWith('///') && !l.trimStart().startsWith('//'))
    .join('\n');
}
function bodyAfter(src, from) {
  const open = src.indexOf('{', from);
  let depth = 0;
  for (let i = open; i < src.length; i++) {
    if (src[i] === '{') depth++;
    else if (src[i] === '}') {
      depth--;
      if (depth === 0) return src.slice(open + 1, i);
    }
  }
  throw new Error('unbalanced braces');
}
// Split on top-level `,` and `;` (TS field separators); braces/parens/brackets
// and the inside of a string literal shield.
function topParts(body) {
  const parts = [];
  let depth = 0;
  let cur = '';
  let quote = null;
  for (const ch of body) {
    if (quote) {
      cur += ch;
      if (ch === quote) quote = null;
      continue;
    }
    if (ch === "'" || ch === '"' || ch === '`') quote = ch;
    if (ch === '{' || ch === '(' || ch === '[') depth++;
    else if (ch === '}' || ch === ')' || ch === ']') depth--;
    if ((ch === ',' || ch === ';') && depth === 0) {
      if (cur.trim()) parts.push(cur.trim());
      cur = '';
    } else cur += ch;
  }
  if (cur.trim()) parts.push(cur.trim());
  return parts;
}

// --- Rust side -------------------------------------------------------------

function rustEnums(src) {
  const out = {};
  const clean = stripComments(src);
  for (const m of clean.matchAll(/pub enum (\w+)/g)) {
    const name = m[1];
    const pre = clean.slice(Math.max(0, m.index - 300), m.index);
    const serde = [...pre.matchAll(/#\[serde\(([^)]*)\)\]/g)].pop();
    const attrs = serde ? serde[1].split(',') : [];
    const tag = (attrs.find((a) => a.includes('tag')) ?? '').split('=')[1]?.trim().replace(/"/g, '');
    const rename = (attrs.find((a) => a.includes('rename_all')) ?? '').split('=')[1]?.trim().replace(/"/g, '');
    const variants = [];
    for (const part of topParts(bodyAfter(clean, m.index))) {
      // Serde/allow attributes sit on their own line above a variant; they
      // must not glue into the variant part the field split sees.
      const p2 = part
        .split('\n')
        .filter((l) => !l.trimStart().startsWith('#'))
        .join(' ')
        .trim();
      const vm = p2.match(/^(\w+)(\s*\{[^]*\})?$/);
      if (!vm) continue;
      const fields = {};
      if (vm[2]) {
        const inner = vm[2].match(/\{([\s\S]*)\}/)[1];
        for (const f of topParts(inner)) {
          const fmm = f.match(/^(\w+)\s*:\s*([\s\S]+)$/);
          if (fmm) fields[fmm[1]] = fmm[2].trim();
        }
      }
      variants.push({ name: vm[1], tag: rename === 'lowercase' ? vm[1].toLowerCase() : snake(vm[1]), fields });
    }
    out[name] = { tag: tag ?? 'kind', variants };
  }
  return out;
}

function rustStructs(src) {
  const out = {};
  const clean = stripComments(src);
  for (const m of clean.matchAll(/pub struct (\w+)\s*\{/g)) {
    const name = m[1];
    const body = bodyAfter(clean, m.index);
    const fields = {};
    for (const part of topParts(body)) {
      const p2 = part
        .split('\n')
        .filter((l) => !l.trimStart().startsWith('#'))
        .join(' ')
        .replace(/^\s*pub\s+/, '');
      const fm = p2.match(/^(\w+)\s*:\s*([\s\S]+)$/);
      if (!fm) continue;
      const preField = body.slice(Math.max(0, body.indexOf(`${fm[1]}:`) - 200), body.indexOf(`${fm[1]}:`));
      const attrs = [...preField.matchAll(/#\[serde\(([^)]*)\)\]/g)];
      const last = attrs.length ? attrs[attrs.length - 1][1].split(',') : [];
      fields[fm[1]] = { type: fm[2].trim(), skip: last.some((a) => a.includes('skip')) };
    }
    out[name] = fields;
  }
  return out;
}

// --- TS side ---------------------------------------------------------------

function tsKinds(src) {
  const out = {};
  const clean = stripComments(src);
  for (const m of clean.matchAll(/export (type|interface) (\w+)/g)) {
    const name = m[2];
    let body;
    const lineEnd = clean.indexOf('\n', m.index);
    const isInterface = clean.slice(m.index, lineEnd).includes('{');
    if (isInterface) {
      body = bodyAfter(clean, m.index);
    } else {
      const eq = clean.indexOf('=', m.index);
      let depth = 0;
      let end = -1;
      for (let i = eq; i < clean.length; i++) {
        if (clean[i] === '{' || clean[i] === '(' || clean[i] === '[') depth++;
        else if (clean[i] === '}' || clean[i] === ')' || clean[i] === ']') depth--;
        else if (clean[i] === ';' && depth === 0) {
          end = i;
          break;
        }
      }
      if (end < 0) throw new Error(`unterminated type ${name}`);
      body = clean.slice(eq + 1, end);
    }
    if (isInterface) {
      out[name] = { kind: 'struct', fields: parseFields(body) };
      continue;
    }
    const alts = splitAlt(body);
    if (alts.every((a) => a.startsWith("'") || a.startsWith('"'))) {
      out[name] = { kind: 'enum', values: alts.map((a) => a.replace(/^["']|["']$/g, '')) };
      continue;
    }
    if (alts.length === 1 && alts[0].startsWith('{')) {
      out[name] = { kind: 'struct', fields: parseFields(alts[0].trim().slice(1, -1)) };
      continue;
    }
    const members = alts.map((a) => {
      a = a.trim();
      if (!a.startsWith('{')) return { tagFields: {}, fields: {}, named: a };
      const inner = a.replace(/^\{|\}$/g, '');
      const fields = parseFields(inner);
      const tagFields = {};
      for (const [k, f] of Object.entries(fields)) if (f.literal) tagFields[k] = f.literal;
      return { tagFields, fields, named: null };
    });
    out[name] = { kind: 'union', members };
  }
  return out;
}

function parseFields(body) {
  const fields = {};
  for (const p of topParts(body)) {
    const fm = p.match(/^(\w+)\??\s*:\s*([\s\S]+)$/);
    if (!fm) continue;
    const t = fm[2].trim().replace(/\s*\|\s*undefined$/, '');
    fields[fm[1]] = {
      type: t,
      optional: p.startsWith(fm[1] + '?'),
      literal: t.startsWith("'") && t.endsWith("'") && !t.includes('|') ? t.slice(1, -1) : undefined
    };
  }
  return fields;
}

// Split a type body on top-level `|` (union alternatives).
function splitAlt(body) {
  const parts = [];
  let depth = 0;
  let cur = '';
  let quote = null;
  for (const ch of body) {
    if (quote) {
      cur += ch;
      if (ch === quote) quote = null;
      continue;
    }
    if (ch === "'" || ch === '"') quote = ch;
    if (ch === '{' || ch === '(' || ch === '[') depth++;
    else if (ch === '}' || ch === ')' || ch === ']') depth--;
    if (ch === '|' && depth === 0) {
      if (cur.trim()) parts.push(cur.trim());
      cur = '';
    } else cur += ch;
  }
  if (cur.trim()) parts.push(cur.trim());
  return parts;
}

// --- type normalization ----------------------------------------------------

const rustE = rustEnums(rustSrc);
const rustS = rustStructs(rustSrc);
const tsK = tsKinds(tsSrc);

function normRust(t) {
  t = t.trim().replace(/^snapshot::/, '').replace(/^crate::/, '').replace(/\s+/g, ' ');
  if (t === 'String') return 'string';
  if (t === 'bool') return 'boolean';
  if (/^(u8|u16|u32|u64|i8|i16|i32|i64|usize|isize|f32|f64)$/.test(t)) return 'number';
  if (t === 'Value') return 'unknown';
  const opt = t.match(/^Option<(.+)>$/);
  if (opt) {
    const inner = normRust(opt[1]);
    if (inner === 'unknown') return 'unknown';
    return `${inner} | null`;
  }
  const vec = t.match(/^Vec<(.+)>$/);
  if (vec) return `${normRust(vec[1])}[]`;
  if (ALIASES[t]) return ALIASES[t];
  return t;
}
function normTs(t) {
  t = t.trim().replace(/\s+/g, ' ');
  if (t.startsWith('{') && t.endsWith('}')) {
    const fields = {};
    for (const p of topParts(t.slice(1, -1))) {
      const fm = p.match(/^(\w+)\??:\s*([\s\S]+)$/);
      if (fm) fields[fm[1]] = fm[2].trim().replace(/\s*\|\s*undefined$/, '');
    }
    return `obj(${Object.entries(fields)
      .map(([k, v]) => `${k}: ${normTs(v)}`)
      .sort()
      .join(', ')})`;
  }
  if (t.endsWith('[]')) return `${normTs(t.slice(0, -2))}[]`;
  if (t.includes('|')) return t.split('|').map((s) => normTs(s.trim())).join(' | ');
  return t;
}
const isLiteralUnion = (t) => t.split(' | ').every((s) => s.startsWith("'") && s.endsWith("'"));
function sameType(a, b) {
  if (a === b) return true;
  if (a === 'unknown' || a === 'unknown[]' || b === 'unknown' || b === 'unknown[]') return true;
  if (a === 'string' && isLiteralUnion(b)) return true;
  if (rustS[a] && b.startsWith('obj(')) return expandRustStruct(a) === b;
  if (tsK[b]?.kind === 'struct' && a.startsWith('obj(')) return `obj(${Object.entries(tsK[b].fields)
    .map(([k, v]) => `${k}: ${normTs(v.type)}`)
    .sort()
    .join(', ')})` === a;
  // A Rust simple enum may be mirrored inline as a TS literal union of the
  // same tags (Event.om_status.kind mirrors OmStatusKind).
  if (rustE[a] && isLiteralUnion(b)) {
    const rv = rustE[a].variants.map((v) => v.tag);
    const tv = b.split(' | ').map((s) => s.slice(1, -1));
    return rv.length === tv.length && rv.every((v) => tv.includes(v));
  }
  return false;
}
function expandRustStruct(name) {
  return `obj(${Object.entries(rustS[name])
    .map(([k, v]) => `${k}: ${normRust(v.type)}`)
    .sort()
    .join(', ')})`;
}

// --- the diffs --------------------------------------------------------------

function diffUnion(rustName, tsName = rustName) {
  const r = rustE[rustName];
  const t = tsK[tsName];
  if (!r) return;
  if (!t || t.kind !== 'union') {
    errors.push(`${rustName}: missing TS union mirror`);
    return;
  }
  const rtags = r.variants.map((v) => v.tag);
  const ttags = t.members.map((m) => m.tagFields[r.tag]).filter((x) => x !== undefined);
  for (const tag of rtags) if (!ttags.includes(tag)) errors.push(`${rustName}: missing TS tag '${tag}'`);
  for (const tag of ttags) if (!rtags.includes(tag)) errors.push(`${rustName}: TS-only tag '${tag}'`);
  for (const rv of r.variants) {
    const tv = t.members.find((m) => m.tagFields[r.tag] === rv.tag);
    if (!tv) continue;
    const rfields = Object.entries(rv.fields).map(([n, ty]) => [n, normRust(ty)]);
    const tfields = Object.entries(tv.fields)
      .filter(([n]) => n !== r.tag)
      .map(([n, f]) => [n, normTs(f.type)]);
    const rnames = rfields.map(([n]) => n);
    const tnames = tfields.map(([n]) => n);
    for (const n of rnames) if (!tnames.includes(n)) errors.push(`${rustName}.${rv.tag}: missing TS field '${n}'`);
    for (const n of tnames) if (!rnames.includes(n)) errors.push(`${rustName}.${rv.tag}: TS-only field '${n}'`);
    for (const [n, ty] of rfields) {
      const tt = tfields.find(([x]) => x === n)?.[1];
      if (tt !== undefined && !sameType(ty, tt)) errors.push(`${rustName}.${rv.tag}.${n}: Rust ${ty} vs TS ${tt}`);
    }
  }
}

function diffStruct(rustName, tsName = rustName) {
  const r = rustS[rustName];
  const t = tsK[tsName];
  if (!r) return;
  if (!t || t.kind !== 'struct') {
    errors.push(`${rustName}: missing TS interface mirror`);
    return;
  }
  const rnames = Object.keys(r);
  const tnames = Object.keys(t.fields);
  for (const n of rnames) if (!tnames.includes(n)) errors.push(`${rustName}: missing TS field '${n}'`);
  for (const n of tnames) if (!rnames.includes(n)) errors.push(`${rustName}: TS-only field '${n}'`);
  for (const [n, rf] of Object.entries(r)) {
    const tf = t.fields[n];
    if (!tf) continue;
    const a = normRust(rf.type).replace(/\?$/, '');
    const b = normTs(tf.type);
    if (a === b) continue;
    if (!sameType(a, b)) errors.push(`${rustName}.${n}: Rust ${a} vs TS ${b}`);
  }
}

function diffSimpleEnum(rustName, tsName = rustName) {
  const r = rustE[rustName];
  const t = tsK[tsName];
  if (!r) return;
  if (!t || t.kind !== 'enum') {
    errors.push(`${rustName}: missing TS literal-union mirror`);
    return;
  }
  const rv = r.variants.map((v) => v.tag);
  for (const v of rv) if (!t.values.includes(v)) errors.push(`${rustName}: missing TS value '${v}'`);
  for (const v of t.values) if (!rv.includes(v)) errors.push(`${rustName}: TS-only value '${v}'`);
}

for (const name of Object.keys(rustE)) {
  const t = tsK[ALIASES[name] ?? name];
  if (!t) {
    infos.push(`${name}: no TS mirror found — skipped`);
    continue;
  }
  if (t.kind === 'enum') diffSimpleEnum(name, ALIASES[name] ?? name);
  else if (t.kind === 'union') diffUnion(name, ALIASES[name] ?? name);
}
for (const name of Object.keys(rustS)) {
  const t = tsK[ALIASES[name] ?? name];
  if (!t) {
    infos.push(`${name}: no TS mirror found — skipped`);
    continue;
  }
  if (t.kind === 'struct') diffStruct(name, ALIASES[name] ?? name);
  else infos.push(`${name}: TS mirror is not a struct — skipped`);
}

// C9 payload shapes: diff against same-named Rust types when the crate
// carries them; until C9-rust lands they are TS-only (the interim guard).
for (const name of Object.keys(tsK)) {
  if (!/(Payload|FunctionCall)$/.test(name)) continue;
  if (rustE[name] || rustS[name]) {
    if (rustE[name]) diffUnion(name);
    else diffStruct(name);
  } else {
    infos.push(`${name}: TS-only (no Rust counterpart in the crate yet)`);
  }
}

for (const i of infos) console.log(`skip: ${i}`);
if (errors.length) {
  for (const e of errors) console.error(`drift: ${e}`);
  console.error(`\nprotocol parity: ${errors.length} drift(s)`);
  process.exit(1);
}
console.log(`protocol parity: OK (${Object.keys(rustE).length + Object.keys(rustS).length} Rust surfaces checked)`);
