<script lang="ts">
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
    <button onclick={onclose}>Close</button>
  </div>
  <div class="row">
    <button class:active={mode === 'join'} onclick={() => (mode = 'join')}
      >Join with link</button
    ><button class:active={mode === 'create'} onclick={() => (mode = 'create')}
      >Create space</button
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
        >GUI invitation or enrollment link<input
          type="password"
          bind:value={link}
          required
          autocomplete="off"
          placeholder="phis://server/space#code"
        /></label
      >
      <p class="muted">
        Use a grant issued for a GUI device. Grants for CLI devices cannot
        enroll the desktop app.
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
    <p class="muted">
      This creates a separate GUI identity. Private keys stay in your OS
      keychain.
    </p>
    <button class="primary" type="submit" disabled={busy}
      >{mode === 'join' ? 'Join space' : 'Create space'}</button
    >
  </form>
</section>
