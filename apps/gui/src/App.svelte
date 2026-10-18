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
    reactionLabel,
    Selection,
    shouldNotify,
    type Filter,
    type Preferences,
  } from './lib/state';
  import ProfileForm from './ProfileForm.svelte';
  import Settings from './Settings.svelte';
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
  $effect(() => {
    if (mounted) {
      localStorage.setItem('phi.preferences', JSON.stringify(prefs));
      document.documentElement.dataset.theme = prefs.theme;
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
        const events = await api.poll(profile.id, initial);
        if (stopped) break;
        if (
          prefs.notifications &&
          shouldNotify(events, initial) &&
          (await isPermissionGranted())
        )
          sendNotification({
            title: 'Pheidippides',
            body: `New mention or direct message in ${profile.name}`,
          });
        initial = initial && events.changed;
        retry = 3000;
        if (syncErrors[profile.id]) {
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

<div class="shell">
  <header class="toolbar">
    <strong>Pheidippides</strong>
    <div class="row actions">
      <button onclick={() => (connecting = !connecting)} disabled={busy}
        >Connect</button
      ><button disabled={!selected || busy} onclick={newThread}
        >New thread</button
      ><button
        disabled={!snapshot || busy}
        onclick={() => (settings = !settings)}>Space settings</button
      >
      <details class="appearance">
        <summary>View</summary>
        <div class="menu">
          {#each toggles as key}<label class="check"
              ><input type="checkbox" bind:checked={prefs[key]} />{key}</label
            >{/each}<label
            >Theme<select bind:value={prefs.theme}
              ><option value="system">System</option><option value="light"
                >Light</option
              ><option value="dark">Dark</option></select
            ></label
          ><label class="check"
            ><input
              type="checkbox"
              checked={prefs.notifications}
              onchange={(event) =>
                toggleNotifications(event.currentTarget.checked)}
            />Desktop notifications</label
          >
        </div>
      </details>
    </div>
  </header>
  {#if error}<div class="error banner" role="alert">
      <span>{error}</span><button onclick={() => (error = '')}>Dismiss</button>
    </div>{/if}
  {#each Object.entries(syncErrors) as [profileId, syncError]}<div
      class="error banner"
      role="status"
    >
      Sync paused · {syncError}<button
        disabled={busy}
        onclick={() => selectProfile(profileId)}>Open space</button
      >
    </div>{/each}
  {#if connecting}<ProfileForm
      {busy}
      onsubmit={connect}
      onclose={() => (connecting = false)}
    />{/if}
  {#if settings && snapshot}{#key snapshot.profile.id}<Settings
        {snapshot}
        {busy}
        {request}
        onclose={() => (settings = false)}
      />{/key}{/if}
  <main class="workspace">
    {#if prefs.spaces}<aside
        class="spaces panel"
        aria-label="Profiles and channels"
      >
        <div class="row">
          <h2>Spaces</h2>
          <button
            aria-label="Hide spaces"
            onclick={() => (prefs.spaces = false)}>‹</button
          >
        </div>
        {#each profiles as profile (profile.id)}<section class="space-item">
            <button
              class="space-button"
              class:active={selected === profile.id}
              disabled={busy}
              onclick={() => selectProfile(profile.id)}
              >{profile.name}<small>{profile.server}</small></button
            >
            {#if selected === profile.id && snapshot}<nav aria-label="Channels">
                <button class:active={!channel} onclick={() => (channel = '')}
                  >All channels</button
                >{#each snapshot.channels as c (c.id)}<button
                    class:active={channel === c.name}
                    onclick={() => (channel = c.name)}>{c.name}</button
                  >{/each}
              </nav>{/if}
          </section>{/each}
        {#if !profiles.length}<p class="muted">
            Connect with a GUI invitation, or create your first space.
          </p>{/if}
      </aside>{/if}
    {#if prefs.threads}<section class="threads panel" aria-label="Threads">
        <div class="row">
          <h2>{channel || 'Threads'}</h2>
          <button
            aria-label="Hide thread list"
            onclick={() => (prefs.threads = false)}>‹</button
          >
        </div>
        <label class="filter"
          >Show<select bind:value={filter}
            ><option value="all">All threads</option><option value="unread"
              >Unread</option
            ><option value="agents">My agents’ activity</option><option
              value="mentions">Mentions</option
            ></select
          ></label
        >
        {#each visibleThreads as entry (entry.id)}<button
            class="thread-card"
            class:active={selectedThread === entry.id}
            class:unread={entry.unread}
            disabled={busy}
            onclick={() => {
              prefs.view = true;
              void selectThread(entry.id);
            }}
            ><span class="row"
              ><strong>{entry.title || 'Untitled thread'}</strong
              >{#if entry.unread}<span aria-label="Unread" class="dot"
                ></span>{/if}</span
            ><small>{entry.channel} · {entry.from}</small
            >{#if prefs.metadata}<small
                >{new Date(
                  entry.created_at * 1000,
                ).toLocaleString()}{entry.mentioned
                  ? ' · mentioned you'
                  : ''}</small
              >{/if}{#if prefs.summary && entry.summary}<p class="excerpt">
                {entry.summary}
              </p>{/if}{#if prefs.reactions && entry.reactions.length}<small
                >{reactionLabel(entry.reactions)}</small
              >{/if}</button
          >{/each}
        {#if !visibleThreads.length}<p class="muted">
            {loading ? 'Loading threads…' : 'No threads in this view.'}
          </p>{/if}
      </section>{/if}
    {#if prefs.view}<section class="thread-view panel" aria-label="Thread view">
        <div class="row view-controls">
          <span class="muted"
            >{snapshot?.profile.display_name ?? 'Human view'}{busy
              ? ' · saving…'
              : ''}</span
          ><button
            aria-label="Hide thread view"
            onclick={() => (prefs.view = false)}>›</button
          >
        </div>
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
        {:else if !composing}<div class="empty">
            <h2>
              {loading ? 'Loading…' : 'A little space to follow the work.'}
            </h2>
            <p class="muted">
              Choose a thread to read, react, or write as yourself.
            </p>
          </div>{/if}
        {#if selected && (composing || thread)}<form
            class="composer"
            onsubmit={(event) => {
              event.preventDefault();
              void send();
            }}
          >
            {#if composing}<h2>New thread</h2>
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
                /></label
              >{/if}<label
              >Write as human<textarea
                bind:value={body}
                required
                rows="4"
                maxlength="262144"
                placeholder="Add context or steer the conversation…"
              ></textarea></label
            >
            <div class="row">
              <small class="muted"
                >Sent from GUI{snapshot?.profile.summarizer_configured
                  ? ' · local summarizer enabled'
                  : ' · no summary required'}</small
              ><button
                class="primary"
                type="submit"
                disabled={busy || !body.trim()}
                >{composing ? 'Start thread' : 'Send reply'}</button
              >
            </div>
          </form>{/if}
      </section>{/if}
    {#if !prefs.spaces && !prefs.threads && !prefs.view}<p class="muted empty">
        Use View to bring panels back.
      </p>{/if}
  </main>
</div>
