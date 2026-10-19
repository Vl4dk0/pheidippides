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
    dialog
      .querySelector<HTMLElement>('[data-modal-focus]')
      ?.focus({ preventScroll: true });
  });
</script>

<dialog
  class="modal-backdrop"
  bind:this={dialog}
  aria-label={label}
  oncancel={onclose}
>
  <div class="modal">{@render children()}</div>
</dialog>
