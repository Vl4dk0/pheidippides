<script lang="ts">
  import { untrack } from 'svelte';
  import { grantLink, nothing, type Request, type Snapshot } from './lib/api';
  import { z } from 'zod';
  let {
    snapshot,
    busy,
    request,
    onclose,
  }: {
    snapshot: Snapshot;
    busy: boolean;
    request: <T>(
      action: Request,
      schema: z.ZodType<T>,
    ) => Promise<T | undefined>;
    onclose: () => void;
  } = $props();
  let name = $state(untrack(() => snapshot.profile.display_name));
  let retention = $state(untrack(() => snapshot.retention_days));
  let summaryMode = $state(untrack(() => snapshot.summary_mode));
  let deviceKind = $state<'gui' | 'cli'>('gui');
  let code = $state('');
  let codeType = $state('');
  let confirmation = $state('');
  async function grant(enroll: boolean, recovery: boolean) {
    code = '';
    const result = await request(
      {
        kind: 'grant',
        enroll,
        recovery,
        device_kind: recovery ? 'gui' : deviceKind,
      },
      grantLink,
    );
    if (result) {
      code = result;
      codeType = recovery
        ? 'Recovery code'
        : enroll
          ? 'Add-device code'
          : 'Invitation';
    }
  }
  async function revoke(
    target: Extract<Request, { kind: 'revoke' }>['target'],
    key: string,
  ) {
    if (confirmation !== key) {
      confirmation = key;
      return;
    }
    confirmation = '';
    await request({ kind: 'revoke', target }, nothing);
  }
  function date(value: number | null) {
    return value === null ? 'never' : new Date(value * 1000).toLocaleString();
  }
</script>

<section class="settings panel" aria-label="Space settings">
  <div class="row">
    <h2>{snapshot.profile.name} settings</h2>
    <button onclick={onclose}>Close</button>
  </div>
  <form
    onsubmit={async (event) => {
      event.preventDefault();
      await request({ kind: 'display_name', name }, nothing);
    }}
  >
    <label
      >Display name<input bind:value={name} required maxlength="120" /></label
    >
    <button type="submit" disabled={busy}>Save display name</button>
  </form>
  <form
    onsubmit={async (event) => {
      event.preventDefault();
      await request(
        {
          kind: 'settings',
          retention_days: retention,
          summary_mode: summaryMode,
        },
        nothing,
      );
    }}
  >
    <label
      >Retention in days (0 = forever)<input
        type="number"
        min="0"
        max="4294967295"
        step="1"
        bind:value={retention}
        required
      /></label
    >
    <label
      >Agent summaries<select bind:value={summaryMode}
        ><option value="required">Required</option><option value="off"
          >Off</option
        ></select
      ></label
    >
    <p class="muted">
      Retention removes whole inactive threads. Deletion beyond the server and
      local caches is best-effort.
    </p>
    <button type="submit" disabled={busy}>Save space settings</button>
  </form>
  <p class="muted">
    Local summarizer: {snapshot.profile.summarizer_configured
      ? 'configured; human messages use it'
      : 'not configured; human messages need no summary'}. Configure the command
    in this GUI profile’s profile.toml. It sends plaintext wherever the command
    sends it.
  </p>
  <h3>Members</h3>
  <ul class="plain-list">
    {#each Object.entries(snapshot.members) as [person, displayName] (person)}
      <li class="row">
        <span
          >{displayName}{person === snapshot.profile.person_id
            ? ' (you)'
            : ''}</span
        >
        {#if person !== snapshot.profile.person_id}<button
            class="danger"
            disabled={busy}
            onclick={() =>
              revoke({ kind: 'person', person_id: person }, person)}
            >{confirmation === person ? 'Confirm removal' : 'Remove'}</button
          >{/if}
      </li>
    {/each}
  </ul>
  <label
    >Target application for invitation / device code<select
      bind:value={deviceKind}
      ><option value="gui">GUI desktop</option><option value="cli"
        >CLI / agents</option
      ></select
    ></label
  >
  <div class="row">
    <button disabled={busy} onclick={() => grant(false, false)}
      >Invite member</button
    ><button disabled={busy} onclick={() => grant(true, false)}
      >Add device</button
    >
  </div>
  <h3>Your devices</h3>
  <ul class="plain-list">
    {#each snapshot.devices.filter((device) => device.person_id === snapshot.profile.person_id) as device (device.id)}
      <li>
        <div class="row">
          <span
            >{device.kind} · {device.id.slice(0, 8)}{device.id ===
            snapshot.profile.device_id
              ? ' (this device)'
              : ''}</span
          >
          {#if device.revoked_at === null}<button
              class="danger"
              disabled={busy}
              onclick={() =>
                revoke({ kind: 'device', device_id: device.id }, device.id)}
              >{confirmation === device.id
                ? 'Confirm revocation'
                : 'Revoke'}</button
            >{:else}<span class="muted">Revoked</span>{/if}
        </div>
        <small class="muted">Last seen {date(device.last_seen_at)}</small>
      </li>
    {/each}
  </ul>
  <details>
    <summary>Other members’ devices</summary>
    <ul class="plain-list">
      {#each snapshot.devices.filter((device) => device.person_id !== snapshot.profile.person_id) as device (device.id)}<li
        >
          <div class="row">
            <span
              >{snapshot.members[device.person_id] ?? device.person_id} · {device.kind}
              · {device.id.slice(0, 8)}</span
            >
            {#if device.revoked_at === null}<button
                class="danger"
                disabled={busy}
                onclick={() =>
                  revoke({ kind: 'device', device_id: device.id }, device.id)}
                >{confirmation === device.id
                  ? 'Confirm revocation'
                  : 'Revoke'}</button
              >{:else}<span class="muted">Revoked</span>{/if}
          </div>
          <small class="muted">Last seen {date(device.last_seen_at)}</small>
        </li>{/each}
    </ul>
  </details>
  <p class="muted">
    Removal and revocation protect future messages only. Previously read content
    remains compromised.
  </p>
  <h3>Recovery</h3>
  <p class="muted">
    Export a single-use GUI enrollment code with no expiry. Anyone holding it
    can enroll as you. Store it securely outside phi; it is shown only here.
  </p>
  <button disabled={busy} onclick={() => grant(true, true)}
    >Export recovery code</button
  >
  {#if code}
    <section class="secret" aria-label={codeType}>
      <h3>{codeType}</h3>
      <p>Share outside phi. This code contains a secret.</p>
      <pre>{code}</pre>
      <button onclick={() => (code = '')}>I saved it — hide code</button>
    </section>
  {/if}
</section>
