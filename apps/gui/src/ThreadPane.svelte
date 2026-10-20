<script lang="ts">
  import { tick } from 'svelte';
  import type { Preferences } from './lib/state';
  import type { ReactionKind, Thread } from './lib/api';
  import { relativeTime, dateLabel } from './lib/presentation';
  import IconButton from './IconButton.svelte';
  import Avatar from './Avatar.svelte';
  import {
    CircleDot,
    Sparkles,
    ThumbsUp,
    ThumbsDown,
    CheckCheck,
    Flag,
    Paperclip,
    ChevronDown,
    ArrowDown,
  } from '@lucide/svelte';
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
  const participants = $derived(
    [
      ...new Map(
        thread.messages
          .filter((message) => message.verification_error === null)
          .map((message) => [
            `${message.person_id}/${message.handle}/${message.kind}`,
            message,
          ]),
      ).values(),
    ].sort((a, b) =>
      `${a.author}/${a.handle}`.localeCompare(`${b.author}/${b.handle}`),
    ),
  );
  const reactions = [
    { kind: 'up', label: 'Like', icon: ThumbsUp },
    { kind: 'down', label: 'Dislike', icon: ThumbsDown },
    { kind: 'ack', label: 'Acknowledge', icon: CheckCheck },
    { kind: 'claim', label: 'Claim', icon: Flag },
  ] as const;
  let scrollViewport = $state<HTMLDivElement>();
  let scrollContent = $state<HTMLDivElement>();
  let nearBottom = $state(true);
  let seenMessages = $state(0);
  let following = true;
  let openedThread = '';
  const newMessages = $derived(
    Math.max(0, thread.messages.length - seenMessages),
  );
  function jumpToNewest() {
    if (!scrollViewport) return;
    scrollViewport.scrollTop = scrollViewport.scrollHeight;
    following = true;
    nearBottom = true;
    seenMessages = thread.messages.length;
  }
  function recordScroll() {
    if (!scrollViewport) return;
    nearBottom =
      scrollViewport.scrollHeight -
        scrollViewport.clientHeight -
        scrollViewport.scrollTop <=
      64;
    following = nearBottom;
    if (nearBottom) seenMessages = thread.messages.length;
  }
  // Decide before rendering a refresh, while the reader's previous position still applies.
  $effect.pre(() => {
    const viewport = scrollViewport;
    const id = thread.id;
    const count = thread.messages.length;
    if (openedThread !== id) {
      openedThread = id;
      following = true;
      nearBottom = true;
      seenMessages = count;
    }
    if (viewport && following) {
      void tick().then(() => {
        if (scrollViewport === viewport && thread.id === id && following)
          jumpToNewest();
      });
    }
  });
  $effect(() => {
    if (!scrollViewport || !scrollContent) return;
    // Images, summary toggles and composer resizing can change the bottom after a refresh.
    const observer = new ResizeObserver(() => {
      if (following) jumpToNewest();
    });
    observer.observe(scrollViewport);
    observer.observe(scrollContent);
    return () => observer.disconnect();
  });
  let copyStatus = $state('');
  async function copyPath(path: string, name: string) {
    try {
      await navigator.clipboard.writeText(path);
      copyStatus = `Copied path for ${name}`;
    } catch {
      copyStatus = `Could not copy path: ${path}`;
    }
  }
</script>

<div class="thread-history">
  <div class="thread-scroll" bind:this={scrollViewport} onscroll={recordScroll}>
    <div class="thread-content" bind:this={scrollContent}>
      <header class="thread-heading">
        <div class="row">
          <h2>{thread.title}</h2>
          <IconButton
            icon={CircleDot}
            label="Mark unread"
            disabled={busy}
            onclick={onunread}
          />
        </div>
        {#if prefs.metadata}<p class="participants muted">
            {#each participants as participant}<span
                ><Avatar
                  personId={participant.person_id}
                  author={participant.author}
                  handle={participant.handle}
                  kind={participant.kind}
                  compact
                />{participant.author}{#if participant.kind === 'cli'}/{participant.handle}{/if}</span
              >{/each}
          </p>{/if}
      </header>
      <div class="messages">
        {#each thread.messages as message (message.id)}
          <article
            class="message"
            class:human={message.kind === 'gui'}
            class:failed={message.verification_error !== null}
          >
            <header class="message-header">
              <Avatar
                personId={message.person_id}
                author={message.author}
                handle={message.handle}
                kind={message.kind}
              />
              <div class="identity">
                <strong>{message.author}</strong
                >{#if message.kind === 'cli'}<span class="handle"
                    >/{message.handle}</span
                  >{/if}
              </div>
              {#if prefs.metadata}<span class="message-meta"
                  ><span title="Message number">#{message.number || '?'}</span
                  ><time title={dateLabel(message.created_at)}
                    >{relativeTime(message.created_at)}</time
                  ></span
                >{/if}
            </header>
            {#if message.verification_error}<p class="error" role="alert">
                Verification failed / content unavailable: {message.verification_error}
              </p>
            {:else}
              <div class="message-content">
                <p class="body">{message.body}</p>
                {#if prefs.attachments && message.attachments.length}<ul
                    class="attachments"
                  >
                    {#each message.attachments as attachment}<li>
                        <button
                          class={attachment.preview
                            ? 'image-attachment'
                            : 'file-chip'}
                          title={`Copy local path for ${attachment.name} · ${attachment.mime}`}
                          aria-label={`Copy local path for ${attachment.name}`}
                          onclick={() =>
                            copyPath(attachment.path, attachment.name)}
                        >
                          {#if attachment.preview}<img
                              src={attachment.preview}
                              alt={attachment.name}
                              loading="lazy"
                            />{:else}<Paperclip size={14} /><span
                              >{attachment.name}</span
                            ><small
                              >{Math.ceil(attachment.size / 1024)} KB</small
                            >{/if}
                        </button>
                      </li>{/each}
                  </ul>{/if}
                {#if prefs.reactions}<div
                    class="reaction-bar"
                    class:floating={message.reactions.length === 0}
                    role="toolbar"
                    tabindex="0"
                    aria-label={`Reactions to message ${message.number}`}
                  >
                    {#each reactions as item}
                      {@const own = message.reactions.some(
                        (r) =>
                          r.kind === item.kind &&
                          r.person_id === personId &&
                          (item.kind === 'claim' || r.handle === 'gui'),
                      )}
                      {@const members = message.reactions.filter(
                        (r) => r.kind === item.kind,
                      )}
                      {@const claim =
                        item.kind === 'claim'
                          ? message.reaction_summary.claims[0]
                          : undefined}
                      {@const claimed = !!claim}
                      {@const label = claim
                        ? `${own ? 'Release claim · ' : 'Claimed by '}${claim.author}/${claim.handle}`
                        : `${own ? `Remove ${item.label.toLowerCase()}` : item.label}${members.length ? ` · ${members.map((r) => `${r.author}/${r.handle}`).join(', ')}` : ''}`}
                      <IconButton
                        icon={item.icon}
                        {label}
                        active={own}
                        pressed={own}
                        variant={`${claimed ? 'claim-pill' : 'reaction-pill'} ${members.length ? '' : 'reaction-empty'}`}
                        disabled={busy || (claimed && !own)}
                        onclick={() => onreact(message.id, item.kind, own)}
                        >{#if claim}<span class="claim-author"
                            >{claim.author}<span class="handle"
                              >/{claim.handle}</span
                            ></span
                          >{:else if members.length}<span
                            >{message.reaction_summary[
                              item.kind as 'up' | 'down' | 'ack'
                            ]}</span
                          >{/if}</IconButton
                      >
                    {/each}
                  </div>{/if}
              </div>
            {/if}
          </article>
        {/each}
      </div>
      {#if copyStatus}<p class="copy-status muted" role="status">
          {copyStatus}
        </p>{/if}
    </div>
  </div>
  {#if !nearBottom}
    <IconButton
      icon={ArrowDown}
      label={newMessages
        ? `Jump to newest · ${newMessages} new ${newMessages === 1 ? 'message' : 'messages'}`
        : 'Jump to newest'}
      variant="jump-latest"
      onclick={jumpToNewest}
      >{#if newMessages}<span>{newMessages}</span>{/if}</IconButton
    >
  {/if}
</div>
{#if prefs.summary}<section class="summary-card" aria-label="Thread summary">
    <details open>
      <summary title="Toggle thread summary" aria-label="Toggle thread summary"
        ><Sparkles size={16} /><span>Summary</span><span
          class="summary-coverage"
          title={thread.summary_number === null
            ? 'No coverage yet'
            : `Covers up to message #${thread.summary_number}`}
          >{thread.summary_number === null
            ? '—'
            : `as of #${thread.summary_number}`}</span
        >{#if thread.newer_messages}<span class="muted"
            >/ {thread.newer_messages} newer</span
          >{/if}<ChevronDown size={14} class="summary-chevron" /></summary
      >
      <p class="body">
        {thread.summary ??
          'No summary yet. The next agent reply can update it.'}
      </p>
    </details>
  </section>{/if}
