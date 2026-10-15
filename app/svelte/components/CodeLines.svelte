<script lang="ts">
  import type { RenderLine } from '../lib/diffview';

  let { lines, numbered = false }: { lines: RenderLine[]; numbered?: boolean } = $props();
</script>

{#each lines as rl (rl.key)}
  <div class="line" class:added={rl.mark === 'add'} class:removed={rl.mark === 'del'}>
    {#if numbered}<span class="ln">{rl.no}</span>{/if}
    <span class="mk">{rl.mark === 'add' ? '+' : rl.mark === 'del' ? '−' : ''}</span>
    <span class="src"
      >{#each rl.spans as s (s)}<span style:color={s.color}>{s.text}</span>{/each}</span
    >
  </div>
{/each}

<style>
  .line {
    display: flex;
  }
  .ln {
    flex: none;
    width: calc(var(--lnw, 4) * 1ch + 20px);
    text-align: right;
    padding: 0 8px 0 12px;
    color: var(--faint);
    user-select: none;
  }
  .mk {
    flex: none;
    width: 1.2ch;
    text-align: center;
    color: transparent;
    user-select: none;
  }
  .src {
    flex: 1;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    padding-right: var(--src-pad, 0);
  }
  .line.added {
    background: var(--diff-add-bg);
  }
  .line.added .mk {
    color: var(--diff-add-fg);
  }
  .line.added .ln {
    color: var(--diff-add-fg);
  }
  .line.removed {
    background: var(--diff-del-bg);
  }
  .line.removed .mk {
    color: var(--diff-del-fg);
  }
  .line.removed .src {
    color: var(--diff-del-fg);
  }
  .line.removed .ln {
    color: transparent;
  }
</style>
