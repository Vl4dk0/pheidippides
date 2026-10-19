<script lang="ts">
  import type { Snippet } from 'svelte';
  let {
    label,
    onclose,
    children,
  }: {
    label: string;
    onclose: () => void;
    children: Snippet;
  } = $props();
  let dialog: HTMLDialogElement;
  $effect(() => {
    dialog.showModal();
  });
</script>

<dialog bind:this={dialog} aria-label={label} oncancel={onclose}>
  {@render children()}
</dialog>
