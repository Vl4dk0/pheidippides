<script lang="ts">
  import type { ReactionSummary } from './lib/api';
  let { summary }: { summary: ReactionSummary } = $props();
</script>

{#each ['up', 'down', 'ack'] as kind}
  {@const count = summary[kind as 'up' | 'down' | 'ack']}
  {#if count}<span>{kind} × {count} </span>{/if}
{/each}
{#each summary.claims as claim}<span title={`Message ${claim.message}`}
    >claimed by {claim.author}/{claim.handle}
  </span>{/each}
