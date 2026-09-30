<script lang="ts">
  // Test fixture: gives TreeNode its `label` snippet (a plain JS function
  // cannot be a Svelte 5 snippet — it must be compiled) and forwards the
  // row handlers from the test.
  import TreeNode from '../../components/TreeNode.svelte';

  let {
    text = 'node',
    depth = 0,
    expanded = null,
    selected = false,
    multi = false,
    dimmed = false,
    onRow = undefined,
    onRowDbl = undefined,
    onToggle = undefined,
    onContext = undefined
  }: {
    text?: string;
    depth?: number;
    expanded?: boolean | null;
    selected?: boolean;
    multi?: boolean;
    dimmed?: boolean;
    onRow?: (e: MouseEvent) => void;
    onRowDbl?: () => void;
    onToggle?: () => void;
    onContext?: (e: MouseEvent) => void;
  } = $props();
</script>

<TreeNode
  {depth}
  {expanded}
  {selected}
  {multi}
  {dimmed}
  {onRow}
  {onRowDbl}
  {onToggle}
  {onContext}
>
  {#snippet label()}
    <span class="lbl">{text}</span>
  {/snippet}
</TreeNode>
