// The diagnostic card's sentence (ADR-0034). The core stores a typed fact and
// no display string, so the GUI composes the wording here; a code this build
// does not know renders by its own name, never as a blank card.

import { fmtK } from './format';
import { streamOutcomeLabels } from '../wire/protocol';
import type { AnyEntry, Diagnostic, DiagnosticCode, Entry } from '../wire/protocol';

// A card renders for an older build's spelling too (AnyEntry.kind is a
// free-form string), so the helper takes the union.
export type DiagnosticEntry = Extract<Entry, { kind: 'diagnostic' }> | AnyEntry;

// The two gate codes are different gates: `gate_attempt`/`gate_failed` are the
// completion gate (the check command a session runs at turn end), while
// `context_overflow`/`gate_unavailable` are the Context gate (#227/#233).
const KNOWN_DIAGNOSTIC_CODES: DiagnosticCode[] = [
  'model_changed', 'gate_unavailable', 'context_overflow', 'gate_attempt',
  'gate_failed', 'om_stalled', 'provider_rejected', 'missing_user_query', 'turn_cutoff'
];

// A gate_unavailable record names what the gate lacks; the card says what that
// means, so an undeclared fact and a window with no room do not read as one
// identical gap.
const GATE_GAP: Record<string, string> = {
  context_window: 'context_window undeclared',
  max_tokens: 'max_tokens undeclared',
  context_room: 'the declared window leaves no room once the output cap and the margin are taken'
};

export function diagnosticLabel(entry: DiagnosticEntry): string {
  // A windowed transcript holds snapshot metadata until a paged read hydrates
  // the entry: such a row carries the preview and no payload, and the core's
  // preview for a diagnostic is its code, so the row renders that code instead
  // of composing a sentence from fields it does not have.
  if (!('diagnostic' in entry)) return entry.text || 'unknown';
  const raw: Diagnostic = entry.diagnostic;
  if (!KNOWN_DIAGNOSTIC_CODES.includes(raw.code as DiagnosticCode)) return raw.code;
  const d = raw as Extract<Diagnostic, { code: DiagnosticCode }>;
  const firstLine = (text: string): string => text.split('\n')[0] ?? '';
  switch (d.code) {
    case 'model_changed':
      return d.from
        ? `model: ${d.from} → ${d.to}`
        : `model: ${d.to} (no recorded predecessor)`;
    case 'gate_unavailable':
      return (
        `context gate off for ${d.model}: ` +
        d.missing.map((reason) => GATE_GAP[reason] ?? reason).join('; ')
      );
    case 'context_overflow':
      return `context gate refused: prompt ${fmtK(d.prompt)} over budget ${fmtK(d.budget)}`;
    case 'gate_attempt':
      return `completion gate: ${d.command} failed (repair ${d.retries} of ${d.max})` +
        (d.output ? `: ${firstLine(d.output)}` : '');
    case 'gate_failed':
      return `completion gate: ${d.command} failed after ${d.retries} repairs — marked gate_failed`;
    case 'om_stalled':
      return `compaction stalled: ${fmtK(d.pending)} pending at activation ${fmtK(d.activation)}`;
    case 'provider_rejected':
      return `provider rejected (${d.status}): ${firstLine(d.body)}`;
    case 'missing_user_query':
      return `refused: assembled input has no user query (${d.items} item(s))`;
    case 'turn_cutoff':
      return `provider cut the turn (${streamOutcomeLabels[d.ended_by]}) after ${d.blocks} block(s)`;
    default:
      // Unreachable while the list above covers every DiagnosticCode.
      return raw.code;
  }
}
