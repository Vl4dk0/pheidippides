<script lang="ts">
  import type { Preferences } from './lib/state';
  import ReactionSummaryView from './ReactionSummary.svelte';
  import type { ReactionKind, Thread } from './lib/api';
  let {
    thread,
    prefs,
    busy,
    personId,
    onreact,
    onunread,
  }: {
    thread: Thread;
    prefs: Preferences;
    busy: boolean;
    personId: string;
    onreact: (message: string, kind: ReactionKind, remove: boolean) => void;
    onunread: () => void;
  } = $props();
  const kinds: ReactionKind[] = ['up', 'down', 'ack', 'claim'];
  function date(value: number) {
    return new Date(value * 1000).toLocaleString();
  }
</script>

<header class="thread-heading">
  <div class="row">
    <h2>{thread.title}</h2>
    <button disabled={busy} onclick={onunread}>Mark unread</button>
  </div>
  {#if prefs.metadata}<p class="muted">
      {thread.participants.join(' · ')}
    </p>{/if}
</header>
{#if prefs.summary}<section class="summary-card" aria-label="Thread summary">
    <small class="muted"
      >Rolling summary · {thread.summary_number === null
        ? 'no coverage yet'
        : `as of #${thread.summary_number}`} · {thread.newer_messages}
      newer messages</small
    >
    <p class="body">
      {thread.summary ?? 'No summary yet. The next agent reply can update it.'}
    </p>
  </section>{/if}
<div class="messages">
  {#each thread.messages as message (message.id)}
    <article
      class:human={message.kind === 'gui'}
      class:failed={message.verification_error !== null}
      class="message"
    >
      <header class="row">
        <strong
          >{message.author}{message.kind === 'cli'
            ? `/${message.handle}`
            : ''}</strong
        ><small
          >{message.kind === 'gui' ? 'human · sent from GUI' : 'agent'}</small
        >
      </header>
      {#if prefs.metadata}<small class="muted"
          >#{message.number || '?'} · {date(message.created_at)}</small
        >{/if}
      {#if message.verification_error}
        <p class="error" role="alert">
          Verification failed / content unavailable: {message.verification_error}
        </p>
      {:else}
        <p class="body">{message.body}</p>
        {#if prefs.attachments && message.attachments.length}
          <ul class="attachments plain-list">
            {#each message.attachments as attachment}<li>
                {#if attachment.preview}<img
                    src={attachment.preview}
                    alt={attachment.name}
                    loading="lazy"
                  />{/if}<small
                  >{attachment.name} · {attachment.mime} · {Math.ceil(
                    attachment.size / 1024,
                  )} KB</small
                >
                <details>
                  <summary>Local cache path</summary><code
                    >{attachment.path}</code
                  >
                </details>
              </li>{/each}
          </ul>
        {/if}
        {#if prefs.reactions}
          <div class="reaction-bar">
            {#each kinds as kind}
              {@const own = message.reactions.some(
                (r) =>
                  r.kind === kind &&
                  r.person_id === personId &&
                  (kind === 'claim' || r.handle === 'gui'),
              )}
              {@const claimed =
                kind === 'claim' &&
                message.reactions.some((r) => r.kind === 'claim')}
              <button
                class:active={own}
                aria-pressed={own}
                disabled={busy || (claimed && !own)}
                onclick={() => onreact(message.id, kind, own)}
                >{own && kind === 'claim' ? 'unclaim' : kind}
                {message.reactions.filter((r) => r.kind === kind).length ||
                  ''}</button
              >
            {/each}
          </div>
          {#if message.reactions.length}<small class="muted"
              ><ReactionSummaryView summary={message.reaction_summary} /></small
            >{/if}
        {/if}
      {/if}
    </article>
  {/each}
</div>
