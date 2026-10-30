// The provider rows (the model menu's data, #337): per workspace, refetched
// when the core's config reload changes them. Freshness is the core's — it
// reads the config layers on every `provider_list` and emits
// `provider_changed` when the rows it serves differ — so this module owns
// only the round-trip and the store's copy.
import { command, type ProviderInfo } from '../wire/protocol';
import { store } from './state.svelte';

export async function fetchProviders(ws: string): Promise<ProviderInfo[]> {
  const out = await command({ type: 'provider_list', workspace: ws });
  const rows = out.kind === 'providers' ? out.providers : null;
  if (rows === null) throw new Error('unexpected provider_list output');
  store.providers[ws] = rows;
  return rows;
}
