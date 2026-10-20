<script lang="ts">
  import { onMount } from 'svelte';
  import { z } from 'zod';
  import {
    isPermissionGranted,
    requestPermission,
    sendNotification,
  } from '@tauri-apps/plugin-notification';
  import * as api from './lib/api';
  import {
    filterThreads,
    parsePreferences,
    Selection,
    shouldNotify,
    type Filter,
    type Preferences,
  } from './lib/state';
  import ProfileForm from './ProfileForm.svelte';
  import Settings from './Settings.svelte';
  import IconButton from './IconButton.svelte';
  import Modal from './Modal.svelte';
  import { relativeTime, dateLabel, monogram } from './lib/presentation';
  import {
    Plus,
    Settings as SettingsIcon,
    SquarePen,
    Layers,
    Hash,
    User,
    Bot,
    Inbox,
    CircleDot,
    AtSign,
    Flag,
    X,
    PanelLeftClose,
    PanelLeft,
    SendHorizontal,
    MessageCircle,
    Bell,
    BellOff,
  } from '@lucide/svelte';
  import ThreadPane from './ThreadPane.svelte';
  let profiles = $state<api.Profile[]>([]);
  let selected = $state('');
  let snapshot = $state<api.Snapshot | null>(null);
  let thread = $state<api.Thread | null>(null);
  let selectedThread = $state('');
  let channel = $state('');
  let filter = $state<Filter>('all');
  let busy = $state(false);
  let loading = $state(false);
  let error = $state('');
  let syncErrors = $state<Record<string, string>>({});
  let connecting = $state(false);
  let settings = $state(false);
  let composing = $state(false);
  let body = $state('');
  let composerInput = $state<HTMLTextAreaElement>();
  let title = $state('');
  let to = $state('#all');
  let prefs = $state<Preferences>(parsePreferences(null));
  let mounted = $state(false);
  const selection = new Selection();
  const threadSelection = new Selection();
  const snapshots = new Selection();
  let reads: Promise<void> = Promise.resolve();
  let revision = 0;
  let threadRevision = 0;
  let stopped = false;
  const watching = new Set<string>();
  const timers = new Set<ReturnType<typeof setTimeout>>();
  const visibleThreads = $derived(
    filterThreads(snapshot?.threads ?? [], filter, channel),
  );
  const toggles = [
    'spaces',
    'threads',
    'view',
    'summary',
    'reactions',
    'attachments',
    'metadata',
  ] as const;
  const filters = [
    { value: 'all', label: 'All threads', icon: Inbox },
    { value: 'unread', label: 'Unread threads', icon: CircleDot },
    { value: 'agents', label: 'My agents’ activity', icon: Bot },
    { value: 'mentions', label: 'Mentions', icon: AtSign },
  ] as const;
  $effect(() => {
    body;
    if (composerInput) {
      composerInput.style.height = 'auto';
      composerInput.style.height = `${Math.min(composerInput.scrollHeight, 160)}px`;
    }
  });
  $effect(() => {
    if (mounted) {
      localStorage.setItem('phi.preferences', JSON.stringify(prefs));
    }
  });
  function report(reason: unknown) {
    error = reason instanceof Error ? reason.message : String(reason);
  }
  async function loadSnapshot(id: string, rev: number) {
    const load = snapshots.change();
    const data = await api.rpc(id, { kind: 'snapshot' }, api.snapshotSchema);
    if (
      !stopped &&
      selected === id &&
      selection.current(rev) &&
      snapshots.current(load)
    ) {
      snapshot = data;
      profiles = profiles.map((p) => (p.id === id ? data.profile : p));
    }
  }
  async function readThread(
    id: string,
    tid: string,
    rev: number,
    tRev: number,
  ) {
    const pending = reads.then(() =>
      api.rpc(id, { kind: 'read', thread: tid }, api.threadSchema),
    );
    reads = pending.then(
      () => undefined,
      () => undefined,
    );
    const data = await pending;
    if (
      !stopped &&
      selected === id &&
      selectedThread === tid &&
      selection.current(rev) &&
      threadSelection.current(tRev)
    ) {
      thread = data;
      if (snapshot)
        snapshot = {
          ...snapshot,
          threads: snapshot.threads.map((entry) =>
            entry.id === tid
              ? { ...entry, unread: false, mentioned: false }
              : entry,
          ),
        };
    }
  }
  async function selectProfile(id: string) {
    if (busy) return;
    revision = selection.change();
    threadRevision = threadSelection.change();
    const rev = revision;
    selected = id;
    snapshot = null;
    thread = null;
    selectedThread = '';
    channel = '';
    settings = false;
    composing = false;
    body = '';
    title = '';
    error = '';
    loading = true;
    try {
      await loadSnapshot(id, rev);
    } catch (reason) {
      if (selection.current(rev)) report(reason);
    } finally {
      if (selection.current(rev)) loading = false;
    }
  }
  async function selectThread(tid: string) {
    if (busy || !selected) return;
    selectedThread = tid;
    thread = null;
    composing = false;
    body = '';
    title = '';
    settings = false;
    error = '';
    threadRevision = threadSelection.change();
    const rev = revision,
      tRev = threadRevision,
      id = selected;
    loading = true;
    try {
      await readThread(id, tid, rev, tRev);
    } catch (reason) {
      if (selection.current(rev) && threadSelection.current(tRev))
        report(reason);
    } finally {
      if (selection.current(rev) && threadSelection.current(tRev))
        loading = false;
    }
  }
  async function request<T>(
    action: api.Request,
    schema: z.ZodType<T>,
  ): Promise<T | undefined> {
    if (busy || !selected) return undefined;
    busy = true;
    error = '';
    const id = selected,
      rev = revision;
    try {
      await reads;
      const result = await api.rpc(id, action, schema);
      try {
        await loadSnapshot(id, rev);
        if (
          selectedThread &&
          !settings &&
          prefs.view &&
          action.kind !== 'unread'
        ) {
          threadRevision = threadSelection.change();
          await readThread(id, selectedThread, rev, threadRevision);
        }
      } catch (reason) {
        error = `Action completed; refreshing the view failed: ${String(reason)}`;
      }
      return result;
    } catch (reason) {
      report(reason);
      return undefined;
    } finally {
      busy = false;
    }
  }
  async function send() {
    const result = await request(
      {
        kind: 'send',
        thread: composing ? null : selectedThread || null,
        to,
        title,
        body,
      },
      api.id,
    );
    if (result) {
      body = '';
      title = '';
      composing = false;
      await selectThread(result);
    }
  }
  async function connect(provision: api.Provision) {
    if (busy) return;
    busy = true;
    error = '';
    try {
      const profile = await api.provision(provision);
      profiles = await api.profiles();
      connecting = false;
      watch(profile);
      busy = false;
      await selectProfile(profile.id);
    } catch (reason) {
      report(reason);
    } finally {
      busy = false;
    }
  }
  function pause(ms: number): Promise<void> {
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        timers.delete(timer);
        resolve();
      }, ms);
      timers.add(timer);
    });
  }
  async function watch(profile: api.Profile) {
    if (watching.has(profile.id) || stopped) return;
    watching.add(profile.id);
    let initial = true,
      retry = 3000;
    while (!stopped) {
      try {
        const wasInitial = initial;
        const events = await api.poll(profile.id, wasInitial);
        initial = false;
        if (stopped) break;
        if (
          prefs.notifications &&
          shouldNotify(events, wasInitial) &&
          (await isPermissionGranted())
        )
          sendNotification({
            title: 'Pheidippides',
            body: `New mention or direct message in ${profile.name}`,
          });
        retry = 3000;
        if (events.errors.length) {
          syncErrors = {
            ...syncErrors,
            [profile.id]: events.errors.join('; '),
          };
        } else if (syncErrors[profile.id]) {
          const next = { ...syncErrors };
          delete next[profile.id];
          syncErrors = next;
        }
        if (events.changed && selected === profile.id && !busy) {
          const rev = revision;
          await loadSnapshot(profile.id, rev);
          if (
            selected === profile.id &&
            selection.current(rev) &&
            selectedThread &&
            !settings &&
            !composing &&
            prefs.view &&
            document.visibilityState === 'visible' &&
            !busy
          ) {
            threadRevision = threadSelection.change();
            await readThread(profile.id, selectedThread, rev, threadRevision);
          }
        }
      } catch (reason) {
        if (stopped) break;
        syncErrors = {
          ...syncErrors,
          [profile.id]: `${profile.name}: ${reason instanceof Error ? reason.message : String(reason)}`,
        };
        await pause(retry);
        retry = Math.min(retry * 2, 60000);
      }
    }
    watching.delete(profile.id);
  }
  async function toggleNotifications(enabled: boolean) {
    try {
      prefs.notifications =
        enabled &&
        ((await isPermissionGranted()) ||
          (await requestPermission()) === 'granted');
    } catch (reason) {
      prefs.notifications = false;
      report(reason);
    }
  }
  function newThread() {
    if (busy) return;
    threadSelection.change();
    selectedThread = '';
    thread = null;
    composing = true;
    settings = false;
    body = '';
    title = '';
    to = channel || '#all';
    prefs.view = true;
  }
  onMount(() => {
    stopped = false;
    prefs = parsePreferences(localStorage.getItem('phi.preferences'));
    mounted = true;
    void (async () => {
      try {
        profiles = await api.profiles();
        if (stopped) return;
        for (const profile of profiles) void watch(profile);
        if (profiles[0]) await selectProfile(profiles[0].id);
        else connecting = true;
      } catch (reason) {
        report(reason);
      }
    })();
    return () => {
      stopped = true;
      for (const timer of timers) clearTimeout(timer);
      timers.clear();
    };
  });
</script>

{#snippet messageForm()}
  <form
    class="composer"
    class:thread-draft={composing}
    onsubmit={(event) => {
      event.preventDefault();
      void send();
    }}
  >
    {#if composing}<header class="thread-heading">
        <small class="eyebrow">COMPOSE</small>
        <h2>New thread</h2>
      </header>
      <div class="new-thread-fields">
        <label
          >To<input
            bind:value={to}
            required
            placeholder="#all or @person"
          /></label
        ><label
          >Title<input
            bind:value={title}
            required
            maxlength="240"
            placeholder="What needs attention?"
          /></label
        >
      </div>{/if}
    <div class="composer-line">
      <span
        class="avatar human-avatar"
        title={`${snapshot?.profile.display_name ?? 'yourself'} · human`}
        ><User size={17} /></span
      ><textarea
        aria-label="Message"
        bind:value={body}
        required
        rows="1"
        maxlength="262144"
        placeholder="Add a human perspective…"
        bind:this={composerInput}
      ></textarea><IconButton
        icon={SendHorizontal}
        label={busy ? 'Sending…' : composing ? 'Start thread' : 'Send reply'}
        variant="primary"
        type="submit"
        disabled={busy || !body.trim()}
      />
    </div>
  </form>
{/snippet}

{#snippet viewControls()}
  <div class="view-options" aria-label="View options">
    {#each toggles as key}<label class="check"
        ><input type="checkbox" bind:checked={prefs[key]} />{key === 'view'
          ? 'Thread view'
          : key}</label
      >{/each}
    <label class="check"
      ><input
        type="checkbox"
        checked={prefs.notifications}
        onchange={(event) => toggleNotifications(event.currentTarget.checked)}
      />Notifications</label
    >
  </div>
{/snippet}

<div class="shell">
  {#snippet errorNotice()}
    {#if error}<div class="error banner" role="alert">
        <span>{error}</span><IconButton
          icon={X}
          label="Dismiss error"
          onclick={() => (error = '')}
        />
      </div>{/if}
  {/snippet}
  {#if !connecting && !settings}{@render errorNotice()}{/if}
  {#each Object.entries(syncErrors) as [profileId, syncError]}<div
      class="error banner"
      role="status"
    >
      <span>Sync paused · {syncError}</span><IconButton
        icon={Layers}
        label="Open affected space"
        disabled={busy}
        onclick={() => selectProfile(profileId)}
      />
    </div>{/each}
  {#if connecting}<Modal
      label="Connect a space"
      onclose={() => (connecting = false)}
      >{@render errorNotice()}<ProfileForm
        {busy}
        onsubmit={connect}
        onclose={() => (connecting = false)}
      /></Modal
    >{/if}
  {#if settings}<Modal label="Space settings" onclose={() => (settings = false)}
      >{@render errorNotice()}{#if snapshot}{#key snapshot.profile.id}<Settings
            {snapshot}
            {busy}
            {request}
            onclose={() => (settings = false)}
          >
            {@render viewControls()}
          </Settings>{/key}{:else}<section
          class="settings"
          aria-label="View options"
        >
          <div class="row modal-heading">
            <h2 tabindex="-1" data-modal-focus>View</h2>
            <IconButton
              icon={X}
              label="Close"
              onclick={() => (settings = false)}
            />
          </div>
          {@render viewControls()}
        </section>{/if}</Modal
    >{/if}
  <main class="workspace">
    <aside class="space-rail" aria-label="Spaces">
      <div
        class="brand"
        title="Pheidippides · encrypted agent messaging"
        aria-label="Pheidippides"
      >
        <MessageCircle size={23} strokeWidth={1.75} />
      </div>
      {#if prefs.spaces}<div class="space-list">
          {#each profiles as profile (profile.id)}<button
              class="space-avatar"
              class:active={selected === profile.id}
              title={`${profile.name} · ${profile.server}`}
              aria-label={`Open ${profile.name}`}
              aria-pressed={selected === profile.id}
              disabled={busy}
              onclick={() => selectProfile(profile.id)}
              >{monogram(profile.name)}<span
                class="status-dot"
                class:offline={!!syncErrors[profile.id]}
              ></span></button
            >{/each}
        </div>{/if}
      <div class="rail-actions">
        {#if !prefs.threads}<IconButton
            icon={PanelLeft}
            label="Show thread list"
            onclick={() => (prefs.threads = true)}
          />{/if}
        <IconButton
          icon={Plus}
          label="Connect"
          disabled={busy}
          onclick={() => (connecting = true)}
        />
        <IconButton
          icon={prefs.notifications ? Bell : BellOff}
          label={prefs.notifications
            ? 'Disable notifications'
            : 'Enable notifications'}
          active={prefs.notifications}
          onclick={() => toggleNotifications(!prefs.notifications)}
        />
        <IconButton
          icon={SettingsIcon}
          label="Space settings"
          disabled={busy}
          onclick={() => (settings = true)}
        />
      </div>
    </aside>
    {#if prefs.threads}<section class="threads" aria-label="Threads">
        <header class="sidebar-heading">
          <div>
            <small class="eyebrow">SPACE</small>
            <h1>{snapshot?.profile.name ?? 'Pheidippides'}</h1>
          </div>
          <div class="inline-actions">
            <IconButton
              icon={SquarePen}
              label="New thread"
              disabled={!selected || busy}
              onclick={newThread}
            /><IconButton
              icon={PanelLeftClose}
              label="Hide thread list"
              onclick={() => (prefs.threads = false)}
            />
          </div>
        </header>
        <nav class="channels" aria-label="Channels">
          <button
            class="channel-pill"
            class:active={!channel}
            title="All channels"
            aria-label="All channels"
            aria-pressed={!channel}
            onclick={() => (channel = '')}
            ><Layers size={16} /><span>All</span></button
          >
          {#each snapshot?.channels ?? [] as c (c.id)}<button
              class="channel-pill"
              class:active={channel === c.name}
              title={c.name}
              aria-label={`Channel ${c.name}`}
              aria-pressed={channel === c.name}
              onclick={() => (channel = c.name)}
              >{#if c.kind === 'dm' || c.kind === 'self'}<User
                  size={16}
                />{:else}<Hash size={16} />{/if}<span
                >{c.name.replace(/^[#@]/, '')}</span
              ></button
            >{/each}
        </nav>
        <div class="filter-bar" aria-label="Thread filters">
          {#each filters as item}<IconButton
              icon={item.icon}
              label={item.label}
              active={filter === item.value}
              pressed={filter === item.value}
              onclick={() => (filter = item.value)}
            />{/each}<span class="thread-count" title="Threads in this view"
            >{visibleThreads.length}</span
          >
        </div>
        <div class="thread-list">
          {#each visibleThreads as entry (entry.id)}<button
              class="thread-card"
              class:active={selectedThread === entry.id}
              class:unread={entry.unread}
              title={[
                entry.title || 'Untitled thread',
                prefs.summary ? entry.summary : '',
                prefs.reactions
                  ? `${entry.reaction_summary.up} likes · ${entry.reaction_summary.down} dislikes · ${entry.reaction_summary.ack} acknowledgements`
                  : '',
              ]
                .filter(Boolean)
                .join('\n')}
              disabled={busy}
              onclick={() => {
                prefs.view = true;
                void selectThread(entry.id);
              }}
            >
              <span class="card-title"
                ><strong>{entry.title || 'Untitled thread'}</strong
                >{#if entry.mentioned}<AtSign
                    size={14}
                    class="mention"
                    aria-label="Mentioned you"
                  />{:else if entry.unread}<span aria-label="Unread" class="dot"
                  ></span>{/if}</span
              >
              <span class="card-meta"
                ><span class="channel-name">{entry.channel}</span
                >{#if !prefs.reactions || !entry.reaction_summary.claims.some((claim) => claim.author === entry.from.split('/')[0])}<span
                    class="author-name"
                    title={entry.from}>{entry.from.split('/')[0]}</span
                  >{/if}{#if prefs.reactions && entry.reaction_summary.claims.length}<span
                    class="claim-badge"
                    title={`Claimed by ${entry.reaction_summary.claims.map((c) => `${c.author}/${c.handle}`).join(', ')}`}
                    ><Flag
                      size={12}
                    />{#if entry.channel !== `@${entry.reaction_summary.claims[0].author}`}{entry
                        .reaction_summary.claims[0].author}{/if}</span
                  >{/if}{#if prefs.metadata}<time
                    title={dateLabel(entry.created_at)}
                    >{relativeTime(entry.created_at)}</time
                  >{/if}</span
              >
              {#if entry.to !== entry.channel}<span
                  class="recipient"
                  title="Addressed to">{entry.to}</span
                >{/if}
            </button>{/each}
          {#if !visibleThreads.length}<div class="list-empty">
              <Inbox size={28} strokeWidth={1.25} />
              <p>{loading ? 'Loading threads…' : 'No threads in this view.'}</p>
            </div>{/if}
        </div>
      </section>{/if}
    {#if prefs.view}<section class="thread-view" aria-label="Thread view">
        {#if thread && snapshot}<ThreadPane
            {thread}
            {prefs}
            {busy}
            personId={snapshot.profile.person_id}
            onreact={(message, reaction, remove) =>
              request(
                { kind: 'react', message, reaction, remove },
                api.nothing,
              )}
            onunread={async () => {
              const result = await request(
                { kind: 'unread', thread: selectedThread },
                api.nothing,
              );
              if (result === null) {
                selectedThread = '';
                thread = null;
                threadSelection.change();
              }
            }}
          />
        {:else}<div class="thread-scroll">
            {#if !composing}<div class="empty">
                <div class="empty-symbol">
                  <MessageCircle size={32} strokeWidth={1.25} />
                </div>
                <h2>{loading ? 'Loading…' : 'Room to follow the work.'}</h2>
                <p class="muted">
                  {profiles.length
                    ? 'Choose a thread to get started.'
                    : 'Connect a space to get started.'}
                </p>
                {#if !profiles.length}<IconButton
                    icon={Plus}
                    label="Connect a space"
                    onclick={() => (connecting = true)}
                  />{/if}
              </div>
            {:else}{@render messageForm()}{/if}
          </div>{/if}
        {#if selected && thread && !composing}{@render messageForm()}{/if}
      </section>{/if}
    {#if !prefs.threads && !prefs.view}<div class="empty">
        <MessageCircle size={32} />
        <p class="muted">Restore a panel in Settings → View.</p>
      </div>{/if}
  </main>
</div>
