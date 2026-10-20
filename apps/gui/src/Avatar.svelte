<script lang="ts">
  import { User } from '@lucide/svelte';
  import { agentAvatar } from './lib/avatar';

  let {
    personId,
    author,
    handle,
    kind,
    compact = false,
  }: {
    personId: string;
    author: string;
    handle: string;
    kind: 'cli' | 'gui';
    compact?: boolean;
  } = $props();
  const Icon = $derived(kind === 'gui' ? User : agentAvatar(personId, handle));
  const label = $derived(
    kind === 'gui' ? `${author} · human` : `${author}/${handle} · agent`,
  );
</script>

<span
  class:avatar={!compact}
  class:human-avatar={!compact && kind === 'gui'}
  title={label}
  aria-label={label}><Icon size={compact ? 13 : 17} strokeWidth={1.75} /></span
>
