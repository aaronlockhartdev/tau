<script lang="ts">
  // The expanded tool output. edit renders as a colored git-style diff (the
  // core's bounded-diff contract); read/write render as syntax-highlighted file
  // content; every other tool stays a plain pre. Highlighting is async (shiki)
  // and guarded so a slow recompute never clobbers a newer one.
  import {
    assemble,
    editDiffCode,
    editDiffToDiffLines,
    langFor,
    parseEditDiff,
    type RenderLine
  } from '../lib/diffview';
  import { highlightLines } from '../lib/highlight';

  let { text, name, path }: { text: string; name: string; path?: string } = $props();

  const lang = $derived(langFor(path ?? ''));

  let header = $state('');
  let truncated = $state('');
  let lines = $state<RenderLine[]>([]);

  $effect(() => {
    const t = text;
    const n = name;
    const l = lang;
    if (n !== 'edit' && n !== 'read' && n !== 'write') return;
    let live = true;
    void (async () => {
      if (n === 'edit') {
        const d = parseEditDiff(t);
        if (!d) {
          header = '';
          truncated = '';
          lines = [];
          return;
        }
        header = d.header;
        truncated = d.truncated ?? '';
        const tokens = await highlightLines(editDiffCode(d), l);
        if (!live) return;
        lines = assemble(editDiffToDiffLines(d), tokens);
      } else {
        // read/write: the whole file. A trailing newline is not a line.
        const fileLines = t.split('\n');
        if (fileLines.length > 0 && fileLines[fileLines.length - 1] === '') fileLines.pop();
        const tokens = await highlightLines(fileLines.join('\n'), l);
        if (!live) return;
        lines = assemble(
          fileLines.map((line, i) => ({ kind: 'context', text: line, newIdx: i })),
          tokens
        );
      }
    })();
    return () => {
      live = false;
    };
  });
</script>

{#if name === 'edit' || name === 'read' || name === 'write'}
  {#if header}
    <div class="to-hd">{header}</div>
  {/if}
  <div class="to-code">
    {#each lines as rl (rl.key)}
      <div class="line" class:added={rl.mark === 'add'} class:removed={rl.mark === 'del'}>
        <span class="mk">{rl.mark === 'add' ? '+' : rl.mark === 'del' ? '−' : ''}</span>
        <span class="src"
          >{#each rl.spans as s (s)}<span style:color={s.color}>{s.text}</span>{/each}</span
        >
      </div>
    {/each}
  </div>
  {#if truncated}
    <div class="to-trunc">{truncated}</div>
  {/if}
{:else}
  <pre>{text}</pre>
{/if}

<style>
  pre {
    margin: 0;
    padding: 8px;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 6px;
    font: 11.5px/1.5 var(--mono);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    color: var(--dim);
  }
  .to-hd {
    margin: 0 0 4px;
    font: 11px var(--mono);
    color: var(--faint);
  }
  /* The code body: shiki's dark-plus foreground by default; per-token spans
     override with the grammar's colors (the file-content view's look). */
  .to-code {
    margin: 0;
    padding: 8px;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 6px;
    font: 11.5px/1.5 var(--mono);
    color: var(--code-fg);
  }
  .line {
    display: flex;
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
  }
  .line.added {
    background: var(--diff-add-bg);
  }
  .line.added .mk {
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
  .to-trunc {
    margin-top: 4px;
    font: 11px var(--mono);
    color: var(--faint);
  }
</style>
