<script lang="ts">
  import { untrack } from 'svelte';
  import IconButton from './IconButton.svelte';
  import {
    X,
    Save,
    SlidersHorizontal,
    Users,
    Monitor,
    KeyRound,
    UserPlus,
    Plus,
    UserMinus,
    ShieldX,
    Check,
    EyeOff,
    Download,
    Bot,
    User,
    ChevronDown,
  } from '@lucide/svelte';
  import { relativeTime, dateLabel } from './lib/presentation';
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
  let summaryEvery = $state(untrack(() => snapshot.summary_every));
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
  let tab = $state<'general' | 'members' | 'devices' | 'recovery'>('general');
  const tabs = [
    { key: 'general', label: 'General', icon: SlidersHorizontal },
    { key: 'members', label: 'Members', icon: Users },
    { key: 'devices', label: 'Devices', icon: Monitor },
    { key: 'recovery', label: 'Recovery', icon: KeyRound },
  ] as const;
</script>

<section class="settings panel" aria-label="Space settings">
  <div class="row modal-heading">
    <div>
      <small class="eyebrow">SPACE SETTINGS</small>
      <h2>{snapshot.profile.name}</h2>
    </div>
    <IconButton icon={X} label="Close" onclick={onclose} />
  </div>
  <nav class="settings-tabs" aria-label="Settings sections">
    {#each tabs as item}{@const Icon = item.icon}<button
        class:active={tab === item.key}
        title={item.label}
        aria-label={item.label}
        aria-pressed={tab === item.key}
        onclick={() => (tab = item.key)}
        ><Icon size={16} /><span>{item.label}</span></button
      >{/each}
  </nav>
  {#if tab === 'general'}
    <form
      class="name-form"
      onsubmit={async (event) => {
        event.preventDefault();
        await request({ kind: 'display_name', name }, nothing);
      }}
    >
      <label
        >Display name<input bind:value={name} required maxlength="120" /></label
      ><IconButton
        icon={Save}
        label="Save display name"
        type="submit"
        disabled={busy}
      />
    </form>
    <form
      class="settings-form"
      onsubmit={async (event) => {
        event.preventDefault();
        await request(
          {
            kind: 'settings',
            retention_days: retention,
            summary_mode: summaryMode,
            summary_every: summaryEvery,
          },
          nothing,
        );
      }}
    >
      <div class="settings-fields">
        <label
          >Retention · days<input
            type="number"
            min="0"
            max="4294967295"
            step="1"
            bind:value={retention}
            required
          /></label
        ><label
          >Agent summaries<span class="select-field"
            ><select bind:value={summaryMode}
              ><option value="required">Required</option><option value="off"
                >Off</option
              ></select
            ><ChevronDown size={14} /></span
          ></label
        ><label
          >Summary interval<input
            type="number"
            min="1"
            max="4294967295"
            step="1"
            bind:value={summaryEvery}
            required
            disabled={summaryMode === 'off'}
          /></label
        >
      </div>
      <div class="form-footer">
        <small class="muted"
          >Delete inactive threads after this many days; 0 keeps them forever.
          Summaries refresh every N messages.</small
        ><IconButton
          icon={Save}
          label="Save space settings"
          variant="primary"
          type="submit"
          disabled={busy}
        />
      </div>
    </form>
    <details class="settings-note">
      <summary
        title="Local summarizer details"
        aria-label="Local summarizer details"
        ><Bot size={15} />Local summarizer</summary
      >
      <p class="muted">
        {snapshot.profile.summarizer_configured
          ? 'Configured; human messages use it.'
          : 'Not configured; human messages need no summary.'} Configure the command
        in this GUI profile’s profile.toml. It sends plaintext wherever the command
        sends it.
      </p>
    </details>
    <p class="muted fine-print">
      Retention deletes whole inactive threads. Deletion beyond the server and
      local caches is best-effort.
    </p>
  {:else if tab === 'members'}
    <div class="section-heading">
      <h3>
        Members <span class="muted">{Object.keys(snapshot.members).length}</span
        >
      </h3>
      <IconButton
        icon={UserPlus}
        label="Invite member"
        disabled={busy}
        onclick={() => grant(false, false)}
      />
    </div>
    <ul class="plain-list">
      {#each Object.entries(snapshot.members) as [person, displayName] (person)}<li
          class="row"
        >
          <span class="member-identity"
            ><span class="avatar human-avatar"><User size={16} /></span
            >{displayName}{#if person === snapshot.profile.person_id}<small
                class="muted">you</small
              >{/if}</span
          >{#if person !== snapshot.profile.person_id}<IconButton
              icon={confirmation === person ? Check : UserMinus}
              variant="danger"
              label={confirmation === person
                ? `Confirm removal of ${displayName}`
                : `Remove ${displayName}`}
              disabled={busy}
              onclick={() =>
                revoke({ kind: 'person', person_id: person }, person)}
            />{/if}
        </li>{/each}
    </ul>
    <p class="muted fine-print">
      Removal protects future messages only. Previously read content remains
      compromised.
    </p>
  {:else if tab === 'devices'}
    <div class="section-heading">
      <h3>Your devices</h3>
      <IconButton
        icon={Plus}
        label="Add device"
        disabled={busy}
        onclick={() => grant(true, false)}
      />
    </div>
    <ul class="plain-list">
      {#each snapshot.devices.filter((device) => device.person_id === snapshot.profile.person_id) as device (device.id)}<li
          class="row"
        >
          <div class="device-identity">
            <span class="avatar"
              >{#if device.kind === 'gui'}<Monitor size={16} />{:else}<Bot
                  size={16}
                />{/if}</span
            >
            <div>
              <span class="mono">{device.id.slice(0, 8)}</span
              >{#if device.id === snapshot.profile.device_id}<small
                  class="muted"
                >
                  · this device</small
                >{/if}<small
                class="device-time muted"
                title={device.last_seen_at === null
                  ? 'Never seen'
                  : dateLabel(device.last_seen_at)}
                >Last seen {device.last_seen_at === null
                  ? 'never'
                  : relativeTime(device.last_seen_at)}</small
              >
            </div>
          </div>
          {#if device.revoked_at === null}<IconButton
              icon={confirmation === device.id ? Check : ShieldX}
              variant="danger"
              label={confirmation === device.id
                ? 'Confirm revocation'
                : 'Revoke device'}
              disabled={busy}
              onclick={() =>
                revoke({ kind: 'device', device_id: device.id }, device.id)}
            />{:else}<span class="muted">Revoked</span>{/if}
        </li>{/each}
    </ul>
    <details class="settings-note">
      <summary
        title="Other members’ devices"
        aria-label="Other members’ devices"
        ><Monitor size={15} />Other members’ devices</summary
      >
      <ul class="plain-list">
        {#each snapshot.devices.filter((device) => device.person_id !== snapshot.profile.person_id) as device (device.id)}<li
            class="row"
          >
            <div>
              <span
                >{snapshot.members[device.person_id] ?? device.person_id}</span
              ><small class="device-time muted"
                >{device.kind} ·
                <span class="mono">{device.id.slice(0, 8)}</span>
                · {device.last_seen_at === null
                  ? 'never seen'
                  : `seen ${relativeTime(device.last_seen_at)} ago`}</small
              >
            </div>
            {#if device.revoked_at === null}<IconButton
                icon={confirmation === device.id ? Check : ShieldX}
                variant="danger"
                label={confirmation === device.id
                  ? 'Confirm revocation'
                  : 'Revoke device'}
                disabled={busy}
                onclick={() =>
                  revoke({ kind: 'device', device_id: device.id }, device.id)}
              />{:else}<span class="muted">Revoked</span>{/if}
          </li>{/each}
      </ul>
    </details>
    <p class="muted fine-print">
      Revocation protects future messages only. Previously read content remains
      compromised.
    </p>
  {:else}
    <div class="recovery-intro">
      <span class="empty-symbol"><KeyRound size={28} /></span>
      <h3>Keep a way back in.</h3>
      <p class="muted">
        Export a single-use enrollment code with no expiry. Anyone holding it
        can enroll as you. Store it securely; it is shown only here.
      </p>
      <IconButton
        icon={Download}
        label="Export recovery code"
        variant="primary"
        disabled={busy}
        onclick={() => grant(true, true)}
      />
    </div>
  {/if}
  {#if code}<section class="secret" aria-label={codeType}>
      <div class="row">
        <h3>{codeType}</h3>
        <IconButton
          icon={EyeOff}
          label="I saved it — hide code"
          onclick={() => (code = '')}
        />
      </div>
      <p>Share only with its intended recipient. This code grants access.</p>
      <pre>{code}</pre>
    </section>{/if}
</section>
