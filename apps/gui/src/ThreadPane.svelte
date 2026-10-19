<script lang="ts">
  import type { Preferences } from './lib/state';
  import type { ReactionKind, Thread } from './lib/api';
  import { relativeTime, dateLabel } from './lib/presentation';
  import IconButton from './IconButton.svelte';
  import {
    Bot,
    User,
    CircleDot,
    Sparkles,
    ThumbsUp,
    ThumbsDown,
    CheckCheck,
    Flag,
    Paperclip,
    ChevronDown,
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
  const reactions = [
    { kind: 'up', label: 'Like', icon: ThumbsUp },
    { kind: 'down', label: 'Dislike', icon: ThumbsDown },
    { kind: 'ack', label: 'Acknowledge', icon: CheckCheck },
    { kind: 'claim', label: 'Claim', icon: Flag },
  ] as const;
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
      {#each thread.participants as participant}<span
          ><Bot size={13} />{participant}</span
        >{/each}
    </p>{/if}
</header>
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
            : `#${thread.summary_number}`}</span
        >{#if thread.newer_messages}<span class="muted"
            >+{thread.newer_messages} new</span
          >{/if}<ChevronDown size={14} class="summary-chevron" /></summary
      >
      <p class="body">
        {thread.summary ??
          'No summary yet. The next agent reply can update it.'}
      </p>
    </details>
  </section>{/if}
<div class="messages">
  {#each thread.messages as message (message.id)}
    <article
      class="message"
      class:human={message.kind === 'gui'}
      class:failed={message.verification_error !== null}
    >
      <header class="message-header">
        <span
          class="avatar"
          class:human-avatar={message.kind === 'gui'}
          title={message.kind === 'gui'
            ? 'Human · sent from GUI (device label)'
            : 'Agent'}
          aria-label={message.kind === 'gui' ? 'GUI device' : 'Agent'}
          >{#if message.kind === 'gui'}<User
              size={17}
              strokeWidth={1.75}
            />{:else}<Bot size={17} strokeWidth={1.75} />{/if}</span
        >
        <div class="identity">
          <strong>{message.author}</strong>{#if message.kind === 'cli'}<span
              class="handle">/{message.handle}</span
            >{/if}
        </div>
        {#if prefs.metadata}<span class="message-meta"
            ><span title="Message number">#{message.number || '?'}</span><time
              title={dateLabel(message.created_at)}
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
                  {#if attachment.preview}<img
                      src={attachment.preview}
                      alt={attachment.name}
                      loading="lazy"
                    />{/if}<button
                    class="file-chip"
                    title={`Copy local path for ${attachment.name} · ${attachment.mime}`}
                    aria-label={`Copy local path for ${attachment.name}`}
                    onclick={() => copyPath(attachment.path, attachment.name)}
                    ><Paperclip size={14} /><span>{attachment.name}</span><small
                      >{Math.ceil(attachment.size / 1024)} KB</small
                    ></button
                  >
                </li>{/each}
            </ul>{/if}
          {#if prefs.reactions}<div
              class="reaction-bar"
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
                {@const claimed = item.kind === 'claim' && members.length > 0}
                {@const label = `${own ? (item.kind === 'claim' ? 'Release claim' : `Remove ${item.label.toLowerCase()}`) : item.label}${members.length ? ` · ${members.map((r) => `${r.author}/${r.handle}`).join(', ')}` : ''}`}
                <IconButton
                  icon={item.icon}
                  {label}
                  active={own}
                  pressed={own}
                  variant={claimed ? 'claim-pill' : 'reaction-pill'}
                  disabled={busy || (claimed && !own)}
                  onclick={() => onreact(message.id, item.kind, own)}
                  >{#if claimed}<span class="claim-author"
                      >{members[0].author}<span class="handle"
                        >/{members[0].handle}</span
                      ></span
                    >{:else if members.length}<span>{members.length}</span
                    >{/if}</IconButton
                >
              {/each}
            </div>{/if}
        </div>
      {/if}
    </article>
  {/each}
</div>
{#if copyStatus}<p class="copy-status muted" role="status">{copyStatus}</p>{/if}
