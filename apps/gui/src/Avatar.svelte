<script lang="ts">
  import { User } from '@lucide/svelte';
  import { agentAvatar } from './lib/avatar';

  let {
    personId,
    author,
    handle,
    fromGui,
    compact = false,
  }: {
    personId: string;
    author: string;
    handle: string;
    fromGui: boolean;
    compact?: boolean;
  } = $props();
  const Icon = $derived(fromGui ? User : agentAvatar(personId, handle));
  const label = $derived(
    fromGui ? `${author} · human` : `${author}/${handle} · agent`,
  );
</script>

<span
  class:avatar={!compact}
  class:human-avatar={!compact && fromGui}
  title={label}
  aria-label={label}><Icon size={compact ? 13 : 17} strokeWidth={1.75} /></span
>
