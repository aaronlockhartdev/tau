<script lang="ts">
  // One markdown body: the synchronous base render (plain code blocks)
  // shows immediately, and an async pass upgrades every fenced block with
  // a known language label to shiki's colored spans. A stale highlight
  // never clobbers a newer render: the generation guard drops late results.
  import { md, mdHighlight } from '../lib/markdown';

  let { text }: { text: string } = $props();

  // Initialized empty (not from text: a state initializer may not read a
  // prop); the effect below renders the base synchronously on mount, so the
  // blank lasts a tick (the effect renders the base before paint).
  let html = $state('');
  let gen = 0;

  $effect(() => {
    const t = text;
    const g = ++gen;
    html = md(t);
    void (async () => {
      const out = await mdHighlight(t);
      if (g === gen) html = out;
    })();
  });
</script>

{@html html}
