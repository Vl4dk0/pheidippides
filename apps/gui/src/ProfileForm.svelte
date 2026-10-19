<script lang="ts">
  import IconButton from './IconButton.svelte';
  import { X, Link, Plus, ArrowRight, ShieldCheck } from '@lucide/svelte';
  import type { Provision } from './lib/api';
  let {
    busy,
    onsubmit,
    onclose,
  }: {
    busy: boolean;
    onsubmit: (request: Provision) => void;
    onclose: () => void;
  } = $props();
  let mode = $state<'join' | 'create'>('join');
  let link = $state('');
  let displayName = $state('');
  let server = $state('https://');
  let name = $state('');
  let token = $state('');
</script>

<section class="setup panel" aria-label="Connect a space">
  <div class="row">
    <h2>Connect a space</h2>
    <IconButton icon={X} label="Close" onclick={onclose} />
  </div>
  <div class="mode-switch" aria-label="Connection method">
    <button
      title="Join with link"
      aria-label="Join with link"
      class:active={mode === 'join'}
      aria-pressed={mode === 'join'}
      onclick={() => (mode = 'join')}><Link size={16} />Join</button
    >
    <button
      title="Create space"
      aria-label="Create space"
      class:active={mode === 'create'}
      aria-pressed={mode === 'create'}
      onclick={() => (mode = 'create')}><Plus size={16} />Create</button
    >
  </div>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      onsubmit(
        mode === 'join'
          ? { kind: 'join', link, display_name: displayName }
          : {
              kind: 'create',
              server,
              name,
              display_name: displayName,
              admin_token: token || null,
            },
      );
      link = '';
      token = '';
    }}
  >
    <label
      >Your display name<input
        bind:value={displayName}
        required
        maxlength="120"
        autocomplete="off"
      /></label
    >
    {#if mode === 'join'}
      <label
        >Invitation or enrollment link<input
          type="password"
          bind:value={link}
          required
          autocomplete="off"
          placeholder="phis://server/space#code"
        /></label
      >
      <p class="muted">
        Use a normal invitation to join a space, or a device-add code from any
        of your devices to enroll as the same person.
      </p>
    {:else}
      <label
        >Server origin<input
          type="url"
          bind:value={server}
          required
          placeholder="https://phi.example.com"
        /></label
      >
      <label
        >Space name<input bind:value={name} required maxlength="120" /></label
      >
      <label
        >Server creation token (if required)<input
          type="password"
          bind:value={token}
          autocomplete="off"
        /></label
      >
    {/if}
    <div class="form-footer">
      <small class="muted"
        ><ShieldCheck size={15} />Keys stay in your OS keychain.</small
      ><IconButton
        icon={ArrowRight}
        label={mode === 'join' ? 'Join space' : 'Create space'}
        variant="primary"
        type="submit"
        disabled={busy}
      />
    </div>
  </form>
</section>
