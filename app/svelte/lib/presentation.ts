// The kind → presentation mapping (F3): one pure answer to "how does the
// GUI render this entry" — label, icon, kv rows, and which sections render.
// EntryCard is the generic renderer over the resulting shape, so adding a
// kind touches this module only, and the mapping is testable without
// mounting a component.

import type {
  AnyEntry,
  Entry,
  SkillRef
} from './protocol';
import { argsLines, md, valueLinesOf } from './markdown';
import { splitJsonPayload } from './entries';
import { fmtK } from './format';

// A structured key/value row: the field name on its own line with a colon,
// the value on the lines below.
export interface KvRow {
  k: string;
  lines: string[];
}

// The expansion slots the card persists in the store (one per interactive
// region).
export type ExpandSlot = 'tool' | 'task' | 'output' | 'skill' | 'obs' | 'think';

// One block of a card body (or of the tool's expanded area).
export type Section =
  | { type: 'md'; html: string; dim?: boolean; cls?: 'thinkbody'; label?: string }
  | { type: 'kv'; rows: KvRow[]; block?: boolean }
  | { type: 'text'; text: string; dim?: boolean; whenClosed?: string }
  | { type: 'pre'; text: string; label?: string };

export interface Header {
  icon: string;
  label: string;
  cls: 'sub' | 'par' | 'skill' | 'obs' | 'sys';
  meta: string | null;
  // The slot the header toggles; null = a static header.
  open: ExpandSlot | null;
  // An expandable header opens on a single click (task) or a double
  // click (skill, observation).
  singleClick: boolean;
  caret: boolean;
}

// One top-level element of a card: the user bubble, a quiet state line,
// the thinking line, the tool chip, or the card shell.
export type Shell =
  | { kind: 'bubble'; text: string }
  | { kind: 'stline'; icon: string; label: string }
  | { kind: 'think'; label: string; meta: string | null; openKey: 'think'; body: string }
  | {
    kind: 'tool';
    icon: string;
    label: string;
    summary: string;
    status?: 'ok' | 'running' | 'error';
    openKey: 'tool';
    kv: KvRow[];
    output: { text: string; closed: string; long: boolean } | null;
  }
  | {
    kind: 'card';
    header: Header | null;
    cls: 'obs' | 'interrupted' | null;
    openKey: ExpandSlot | null;
    // The body is hidden while the slot is closed (the task card).
    collapseWhenClosed: boolean;
    sections: Section[];
    // The body revealed when the slot opens; `group` wraps it in the
    // observation-details shell.
    reveal: { sections: Section[]; group: boolean } | null;
    expando: { openKey: ExpandSlot; whenOpen: string; whenClosed: string } | null;
    intmark: boolean;
  };

export interface Presentation {
  shells: Shell[];
}

// The kind-narrowed cases of `present` keep the escape variant
// (AnyEntry.kind is a free-form string), so the helpers take the union.
type ToolEntry = Extract<Entry, { kind: 'tool' }> | AnyEntry;
type SubagentEntry = Extract<Entry, { kind: 'subagent' }> | AnyEntry;
type TaskEntry = Extract<Entry, { kind: 'task' }> | AnyEntry;
type OmEntry = Extract<Entry, { kind: 'om' }> | AnyEntry;


// The user-facing tool output: for bash, the command is prepended to the
// result, and the combined text is what gets truncated.
function toolOutput(e: ToolEntry): string {
  const command =
    e.name === 'bash' && typeof e.args?.command === 'string' ? e.args.command : '';
  return [command, e.output ?? ''].filter(Boolean).join('\n\n');
}

// The chip's one-line summary: the argument that names the operation.
function toolSummary(e: ToolEntry): string {
  const a = e.args;
  if (!a) return e.text ?? '';
  const v = a.command ?? a.file_path ?? a.task ?? a.query ?? a.task_id ?? a.title ?? a.message;
  const s = typeof v === 'string' ? v : '';
  return s ? (s.length > 80 ? s.slice(0, 80) + '…' : s) : JSON.stringify(a);
}

function toolIcon(name: string | undefined): string {
  if (['read', 'write', 'edit'].includes(name ?? '')) return 'i-file';
  if (name?.startsWith('subagent_') || name === 'parent_notify') return 'i-bot';
  if (name === 'recall') return 'i-search';
  if (name?.startsWith('task_')) return 'i-check';
  return 'i-term';
}

function toolShell(e: ToolEntry, out: string): Shell {
  const kv: KvRow[] = e.args
    ? argsLines(e.args)
    : e.text
      ? [{ k: '', lines: [e.text] }]
      : [];
  return {
    kind: 'tool',
    icon: toolIcon(e.name),
    label: e.name ?? '',
    summary: toolSummary(e),
    status: e.status,
    openKey: 'tool',
    kv,
    output: out
      ? {
        text: out,
        closed: out.length > 200 ? out.slice(0, 200) + ' …' : out,
        long: out.length > 200
      }
      : null
  };
}

type Card = Extract<Shell, { kind: 'card' }>;

function card(over: Partial<Omit<Card, 'kind'>> & { sections: Section[] }): Shell {
  return {
    kind: 'card',
    header: null,
    cls: null,
    openKey: null,
    collapseWhenClosed: false,
    reveal: null,
    expando: null,
    intmark: false,
    ...over
  };
}

// A user entry that reports to a peer session (sub-agent child → parent,
// or parent → child): prose with the payload's kv lines, or the raw text.
function reportSections(
  msg: { prose: string; kv: KvRow[] } | null | undefined,
  text: string
): Section[] {
  const sections: Section[] = [];
  if (msg) {
    if (msg.prose) sections.push({ type: 'text', text: msg.prose, dim: true });
    if (msg.kv.length) sections.push({ type: 'kv', rows: msg.kv, block: true });
  } else {
    sections.push({ type: 'text', text, dim: true });
  }
  return sections;
}

function skillCard(skill: SkillRef, text: string): Shell {
  const long = text.length > 200;
  return card({
    header: {
      icon: 'i-book',
      label: `skill · ${skill.name}`,
      cls: 'skill',
      meta: null,
      open: 'skill',
      singleClick: false,
      caret: false
    },
    openKey: 'skill',
    sections: [
      {
        type: 'text',
        text,
        dim: true,
        whenClosed: long ? text.slice(0, 200) + ' …' : text
      }
    ],
    expando: long
      ? {
        openKey: 'skill',
        whenOpen: '▾ hide',
        whenClosed: `▸ full (${text.length} chars)`
      }
      : null
  });
}

function subagentStateLabel(p: SubagentEntry['payload']): string {
  if (p && p.event === 'state') {
    if (p.state === 'idle' && p.waiting_on) return `state: ${p.state} · ${p.waiting_on}`;
    if (p.state === 'failed' && p.reason) return `state: ${p.state} · ${p.reason}`;
    return `state: ${p.state}`;
  }
  return '';
}

function subagentShell(e: SubagentEntry, parentLabel: string): Shell {
  const p = e.payload;
  if (p && p.event === 'state') {
    return { kind: 'stline', icon: 'i-bot', label: subagentStateLabel(p) };
  }
  // The spawn/notify label is the parent's view of a child; the child's
  // own transcript resolves the parent label too (it renders the
  // 'parent · X' row), so the card reads 'spawn' in both views —
  // 'sub-agent' only appears without a parent label.
  const label = parentLabel
    ? p && p.event === 'spawn'
      ? 'spawn'
      : p && p.event === 'notify'
        ? 'reported to parent'
        : 'sub-agent'
    : 'sub-agent';
  const kv: KvRow[] = p
    ? Object.entries(p).map(([k, v]) => ({ k, lines: valueLinesOf(v, 1) }))
    : e.text
      ? [{ k: '', lines: [e.text] }]
      : [];
  return card({
    header: {
      icon: 'i-bot',
      label,
      cls: parentLabel ? 'sys' : 'sub',
      meta: null,
      open: null,
      singleClick: false,
      caret: false
    },
    sections: [{ type: 'kv', rows: kv }]
  });
}

function taskLabel(p: NonNullable<TaskEntry['payload']>): string {
  switch (p.event) {
    case 'created':
      return `task: ${p.title}`;
    case 'started':
      return `task started · ${p.id}`;
    case 'evidence':
      return `evidence · ${p.evidence.criterion}`;
    case 'finished':
      return `task done · ${p.id}`;
    case 'blocked':
      return `task blocked · ${p.id}`;
    case 'assigned':
      return `task assigned · ${p.id}`;
    default:
      return p.event;
  }
}

function taskShell(e: TaskEntry, text: string): Shell {
  const p = e.payload;
  const kv: KvRow[] = p
    ? Object.entries(p)
      .filter(([k]) => k !== 'event')
      .map(([k, v]) => ({ k, lines: valueLinesOf(v, 1) }))
    : text
      ? [{ k: '', lines: [text] }]
      : [];
  return card({
    header: {
      icon: 'i-check',
      label: p ? taskLabel(p) : 'task',
      cls: 'sys',
      meta: null,
      open: 'task',
      singleClick: true,
      caret: true
    },
    openKey: 'task',
    collapseWhenClosed: true,
    sections: [],
    reveal: { sections: [{ type: 'kv', rows: kv }], group: false }
  });
}

function observationShell(e: OmEntry, text: string): Shell {
  // Collapsible like the task card: the body (observation text + details)
  // lives in the reveal, so a closed card is just its header.
  const body: Section[] = [{ type: 'md', html: md(text.trim()), dim: true }];
  if (e.suggestedResponse)
    body.push({ type: 'kv', rows: [{ k: 'suggested', lines: [e.suggestedResponse] }] });
  if (e.thinking)
    body.push({ type: 'md', html: md(e.thinking.trim()), cls: 'thinkbody', label: 'thinking' });
  if (e.input) body.push({ type: 'pre', text: e.input, label: 'input' });
  return card({
    header: {
      icon: 'i-book',
      label: 'observation',
      cls: 'obs',
      meta: e.model ?? null,
      open: 'obs',
      singleClick: true,
      caret: true
    },
    cls: 'obs',
    openKey: 'obs',
    collapseWhenClosed: true,
    sections: [],
    reveal: { sections: body, group: false }
  });
}

// Entry in, presentable shape out. null = a fully empty entry, which
// renders nothing so no shell margin gap is left behind.
export function present(
  entry: Entry,
  ctx: { sourceLabel: string; parentLabel: string }
): Presentation | null {
  const text = entry.text ?? '';
  switch (entry.kind) {
    case 'user': {
      if (!text.trim()) return null;
      if (entry.skill) return { shells: [skillCard(entry.skill, text)] };
      if (entry.source) {
        return {
          shells: [
            card({
              header: {
                icon: 'i-bot',
                label: ctx.sourceLabel ? `sub-agent · ${ctx.sourceLabel}` : 'sub-agent',
                cls: 'sub',
                meta: null,
                open: null,
                singleClick: false,
                caret: false
              },
              // The child → parent split comes decoded (the payload's
              // source); no re-split here.
              sections: reportSections(entry.msg, text)
            })
          ]
        };
      }
      if (ctx.parentLabel) {
        // The parent → child case needs the session's parent link, so the
        // payload is split here.
        return {
          shells: [
            card({
              header: {
                icon: 'i-user',
                label: `parent · ${ctx.parentLabel}`,
                cls: 'par',
                meta: null,
                open: null,
                singleClick: false,
                caret: false
              },
              sections: reportSections(entry.msg ?? splitJsonPayload(text), text)
            })
          ]
        };
      }
      return { shells: [{ kind: 'bubble', text }] };
    }
    case 'message':
    case 'interrupted': {
      const shells: Shell[] = [];
      if (entry.reasoning) {
        shells.push({
          kind: 'think',
          label: 'thinking',
          meta: entry.usage
            ? `${fmtK(entry.usage.input_tokens)} in · ${fmtK(entry.usage.output_tokens)} out`
            : null,
          openKey: 'think',
          body: md(entry.reasoning.trim())
        });
      }
      if (text.trim() || entry.kind === 'interrupted') {
        shells.push(
          card({
            cls: entry.kind === 'interrupted' ? 'interrupted' : null,
            sections: [{ type: 'md', html: md(text.trim()) }],
            intmark: entry.kind === 'interrupted'
          })
        );
      }
      return shells.length ? { shells } : null;
    }
    case 'tool': {
      const out = toolOutput(entry);
      const has =
        text.trim() || entry.status === 'running' || entry.args !== undefined || out.length > 0;
      if (!has) return null;
      return { shells: [toolShell(entry, out)] };
    }
    case 'om':
      if (!text.trim()) return null;
      return { shells: [observationShell(entry, text)] };
    case 'system': {
      if (!text.trim()) return null;
      if (text.startsWith('model: ')) {
        return { shells: [{ kind: 'stline', icon: 'i-bot', label: text }] };
      }
      return {
        shells: [
          card({
            header: {
              icon: 'i-term',
              label: 'system',
              cls: 'sys',
              meta: null,
              open: null,
              singleClick: false,
              caret: false
            },
            sections: [{ type: 'md', html: md(text.trim()), dim: true }]
          })
        ]
      };
    }
    case 'spawn-snapshot':
      if (!text.trim()) return null;
      return {
        shells: [
          card({
            header: {
              icon: 'i-bot',
              label: 'spawn snapshot',
              cls: 'sub',
              meta: null,
              open: null,
              singleClick: false,
              caret: false
            },
            sections: [{ type: 'md', html: md(text.trim()) }]
          })
        ]
      };
    case 'subagent':
      if (!text.trim()) return null;
      return { shells: [subagentShell(entry, ctx.parentLabel)] };
    case 'task':
      if (!text.trim()) return null;
      return { shells: [taskShell(entry, text)] };
    default:
      // An unknown kind renders as a plain text block (no payload
      // interpretation).
      if (!text.trim()) return null;
      return { shells: [card({ sections: [{ type: 'md', html: md(text.trim()) }] })] };
  }
}
