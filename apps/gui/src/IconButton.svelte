<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { LucideIcon } from '@lucide/svelte';
  let {
    icon: Icon,
    label,
    onclick,
    disabled = false,
    active = false,
    pressed,
    type = 'button',
    variant = '',
    children,
  }: {
    icon: LucideIcon;
    label: string;
    onclick?: () => void;
    disabled?: boolean;
    active?: boolean;
    pressed?: boolean;
    type?: 'button' | 'submit';
    variant?: string;
    children?: Snippet;
  } = $props();
</script>

<button
  class={`icon-button ${variant}`}
  class:active
  title={label}
  aria-label={label}
  aria-pressed={pressed}
  {type}
  {disabled}
  {onclick}
>
  <Icon size={18} strokeWidth={1.75} />
  {#if children}{@render children()}{/if}
</button>
