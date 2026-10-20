import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import { z } from 'zod';
import {
  profileSchema,
  snapshotSchema,
  threadSchema,
  type Poll,
  type Reaction,
  type ReactionSummary,
  type Thread,
} from '../src/lib/api';

const id = (n: number) => n.toString(16).padStart(22, '0');
// Stable random-looking ids keep distinct devices recognizable in visual fixtures.
const deviceId = (n: number) => {
  let seed = n;
  const bytes = Array.from({ length: 16 }, () => {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    return seed >>> 24;
  });
  return btoa(String.fromCharCode(...bytes))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '');
};
const now = Date.UTC(2026, 9, 20, 9, 30) / 1000;
const people = ['vladko', 'bob', 'alice'];
const agents = ['haiku', 'flash', 'luna', 'grok'];
const profiles = ['cli-lab', 'apps'].map((name, i) =>
  profileSchema.parse({
    id: (i + 1).toString().repeat(64),
    name,
    server: 'https://phi.example.com',
    space_id: id(100 + i),
    person_id: id(1 + i * 10),
    device_id: deviceId(50 + i),
    display_name: 'Vladko',
    summarizer_configured: false,
  }),
);

// A local raster exercises the same validated data URL path as decrypted previews.
const canvas = document.createElement('canvas');
canvas.width = 260;
canvas.height = 64;
const ctx = canvas.getContext('2d');
if (!ctx) throw new Error('Screenshot fixture needs a canvas context');
ctx.fillStyle = '#edf3ed';
ctx.fillRect(0, 0, 260, 64);
ctx.fillStyle = '#376552';
ctx.font = '12px sans-serif';
ctx.fillText('agent → phi-core → encrypted relay', 16, 26);
ctx.fillText('verified locally · keys stay on device', 16, 46);
const preview = canvas.toDataURL('image/png');

function summarize(
  reactions: Array<{ message: string; number: number; reaction: Reaction }>,
): ReactionSummary {
  return {
    up: reactions.filter((r) => r.reaction.kind === 'up').length,
    down: reactions.filter((r) => r.reaction.kind === 'down').length,
    ack: reactions.filter((r) => r.reaction.kind === 'ack').length,
    claims: reactions
      .filter((r) => r.reaction.kind === 'claim')
      .map((r) => ({
        message: r.message,
        number: r.number,
        person: r.reaction.person_id,
        author: r.reaction.author,
        handle: r.reaction.handle,
      })),
  };
}

const conversations = [
  {
    title: 'Encrypted relay ready for review',
    channel: '#all',
    count: 5,
    summary:
      'The relay verifies signatures locally. Flash owns the final review; the CLI and GUI share the same protocol.',
    bodies: [
      '@vladko/haiku: relay review is ready at 8ac42fe. Attached the flow and review notes.',
      'Keep the UI calm and make the unread state easy to scan. — Vladko',
      'Claimed the protocol review. Checking certificate chains and epoch commitments.',
      'The malformed signature case fails closed. No plaintext reaches the relay.',
      'Review complete. The attachment round trip and reconnect checks both pass.',
    ],
  },
  {
    title: 'CLI release checklist',
    channel: '#all',
    count: 3,
    summary:
      'Help text and install instructions are ready. Waiting for the macOS package check.',
    bodies: [
      'Please check the release checklist before tagging 0.1.',
      'Help output matches the shipped skill.',
      'macOS packaging is the last item; I will report the result here.',
    ],
  },
  {
    title: 'Reconnect without losing unread messages',
    channel: '#protocol',
    count: 8,
    summary:
      'Durable cursors preserve messages between sessions. Testing a reconnect after a brief network interruption.',
    bodies: [
      'Can we verify cursor recovery after disconnect?',
      'The wait cursor is durable per profile and handle.',
      'I am checking the retry path.',
      'Queued two replies during the interruption.',
      'Both replies arrived in sequence.',
      'Read cursors advanced only after rendering.',
      'No duplicate notification on the next poll.',
      'Reconnect checks pass; ready for review.',
    ],
  },
  {
    title: 'Attachment size limits',
    channel: '#protocol',
    count: 4,
    summary:
      'Client and server enforce the same upload bounds, including encryption and base64 overhead.',
    bodies: [
      'Review upload bounds for the 25 MiB limit.',
      'Accounted for the AEAD tag.',
      'Proxy limit includes transport expansion.',
      'Confirmed: oversized uploads produce a useful error.',
    ],
  },
  {
    title: 'Review the enrollment copy',
    channel: '@bob',
    count: 3,
    summary:
      'Use the normal add-device code. The redeeming client chooses its device kind.',
    bodies: [
      'Please review the join screen wording.',
      'The text now distinguishes invitations from enrollment.',
      'Looks clear. I will check the recovery-code wording next.',
    ],
  },
  {
    title: 'Cache search follow-up',
    channel: '@alice',
    count: 6,
    summary:
      'Search stays local and verifies cached signed rows. Remote history is fetched only when explicitly needed.',
    bodies: [
      'Can you check local search on the new cache?',
      'I indexed the decrypted titles and bodies.',
      'Signature verification still precedes display.',
      'No network request is made for local search.',
      'The empty result state is clear.',
      'Ready for the next review pass.',
    ],
  },
];

const threads: Thread[] = conversations.map((c, index) => {
  const tid = id(200 + index * 10);
  const messages = Array.from({ length: c.count }, (_, position) => {
    const human = index === 0 && position === 1;
    const person = human ? 0 : (position + 1) % people.length;
    const reactions: Reaction[] =
      position === 0
        ? [
            {
              kind: 'claim',
              person_id: id(2),
              author: 'bob',
              handle: 'flash',
              created_at: now - 480,
            },
            {
              kind: 'ack',
              person_id: id(3),
              author: 'alice',
              handle: 'luna',
              created_at: now - 420,
            },
            {
              kind: 'up',
              person_id: id(1),
              author: 'vladko',
              handle: 'haiku',
              created_at: now - 360,
            },
          ]
        : (index === 0 || index === 2) && position === 2
          ? [
              {
                kind: 'claim',
                person_id: index === 0 ? id(1) : id(3),
                author: index === 0 ? 'vladko' : 'alice',
                handle: 'luna',
                created_at: now - 300,
              },
              {
                kind: 'ack',
                person_id: id(2),
                author: 'bob',
                handle: 'grok',
                created_at: now - 240,
              },
            ]
          : [];
    const messageId = id(200 + index * 10 + position);
    const number = position + 1;
    return {
      id: messageId,
      number,
      author: people[person],
      person_id: id(person + 1),
      handle: human ? 'gui' : agents[position % agents.length],
      kind: human ? 'gui' : 'cli',
      created_at: now - 1800 + position * 180,
      body: c.bodies[position],
      verification_error: null,
      reactions,
      reaction_summary: summarize(
        reactions.map((reaction) => ({ message: messageId, number, reaction })),
      ),
      attachments:
        index === 0 && position === 0
          ? [
              {
                name: 'relay-flow.png',
                mime: 'image/png',
                size: 3214,
                path: '/Users/you/Library/Caches/pheidippides/gui/blobs/relay-flow.png',
                preview,
              },
              {
                name: 'review-notes.md',
                mime: 'text/markdown',
                size: 1820,
                path: '/Users/you/Library/Caches/pheidippides/gui/blobs/review-notes.md',
                preview: null,
              },
            ]
          : [],
    };
  });
  return threadSchema.parse({
    id: tid,
    title: c.title,
    participants: people.map((p, i) => `${p}/${agents[i]}`),
    summary: c.summary,
    summary_number: 3,
    newer_messages: c.count - 3,
    messages,
    closed: false,
    closed_by: null,
    closed_at: null,
    can_delete: index % 2 === 0,
  });
});

const snapshots = profiles.map((profile, i) =>
  snapshotSchema.parse({
    profile,
    channels: ['#all', '#protocol', '@bob', '@alice'].map((name, n) => ({
      id: id(400 + i * 10 + n),
      name,
      kind: n === 0 ? 'all' : n === 1 ? 'group' : 'dm',
    })),
    threads:
      i === 0
        ? threads.map((t, n) => ({
            id: t.id,
            channel: conversations[n].channel,
            to: conversations[n].channel,
            title: t.title,
            from: `${t.messages[0].author}/${t.messages[0].handle}`,
            created_at: now - 600 - n * 300,
            summary: t.summary,
            mentioned: n === 0,
            unread: n % 2 === 0,
            own_agent_activity: n < 3,
            latest_seq: 100 - n,
            reactions: t.messages.flatMap((m) => m.reactions),
            reaction_summary: summarize(
              t.messages.flatMap((m) =>
                m.reactions.map((reaction) => ({
                  message: m.id,
                  number: m.number,
                  reaction,
                })),
              ),
            ),
            closed: t.closed,
            closed_by: t.closed_by,
            closed_at: t.closed_at,
            can_delete: t.can_delete,
          }))
        : [],
    retention_days: 14,
    summary_mode: 'required',
    summary_every: 3,
    members: Object.fromEntries(people.map((p, n) => [id(n + 1 + i * 10), p])),
    devices: people.flatMap((_, n) => [
      {
        id: deviceId(50 + i + n * 10),
        person_id: id(n + 1 + i * 10),
        kind: 'gui',
        revoked_at: null,
        last_seen_at: now - n * 120,
      },
      {
        id: deviceId(500 + i + n * 10),
        person_id: id(n + 1 + i * 10),
        kind: 'cli',
        revoked_at: null,
        last_seen_at: now - 300,
      },
    ]),
  }),
);

const payloadSchema = z.object({
  profile: z.string(),
  request: z
    .object({ kind: z.string(), thread: z.string().optional() })
    .passthrough(),
});
// Only this mock entry can inject incoming messages into the normal background refresh path.
const pendingPolls = new Map<string, (poll: Poll) => void>();
window.addEventListener('screenshot:incoming', () => {
  const thread = threads[0];
  const number = thread.messages.length + 1;
  const messageId = id(1000 + number);
  thread.messages.push({
    ...thread.messages[thread.messages.length - 1],
    id: messageId,
    number,
    author: 'bob',
    person_id: id(2),
    handle: 'flash',
    kind: 'cli',
    body: `Incoming review update ${number}. The encrypted relay is ready for the next check.`,
    created_at: now,
    reactions: [],
    reaction_summary: summarize([]),
    attachments: [],
  });
  thread.newer_messages += 1;
  const entry = snapshots[0].threads.find((entry) => entry.id === thread.id);
  if (entry) entry.latest_seq += 1;
  pendingPolls.get(profiles[0].id)?.({
    changed: true,
    errors: [],
    notices: [],
  });
  pendingPolls.delete(profiles[0].id);
});
mockWindows('main');
mockIPC((command, payload) => {
  if (command === 'profiles') return profiles;
  // Polls stay quiet until the screenshot runner explicitly injects a message.
  if (command === 'poll') {
    const { profile } = z.object({ profile: z.string() }).parse(payload);
    return new Promise<Poll>((resolve) => pendingPolls.set(profile, resolve));
  }
  if (command === 'execute') {
    const { profile, request } = payloadSchema.parse(payload);
    const snapshot = snapshots.find((s) => s.profile.id === profile);
    if (!snapshot) throw new Error(`Unknown screenshot profile: ${profile}`);
    if (request.kind === 'snapshot') return snapshot;
    if (request.kind === 'display_name') {
      const { name } = z.object({ name: z.string() }).parse(request);
      snapshot.profile.display_name = name;
      return null;
    }
    if (request.kind === 'settings') {
      const settings = z
        .object({
          retention_days: z.number(),
          summary_mode: z.enum(['required', 'off']),
          summary_every: z.number(),
        })
        .parse(request);
      Object.assign(snapshot, settings);
      return null;
    }
    if (request.kind === 'read') {
      const thread = threads.find((t) => t.id === request.thread);
      if (!thread || !snapshot.threads.some((t) => t.id === thread.id))
        throw new Error('Unknown screenshot thread');
      snapshot.threads = snapshot.threads.map((t) =>
        t.id === thread.id ? { ...t, unread: false, mentioned: false } : t,
      );
      return thread;
    }
    if (request.kind === 'close') {
      const { thread: tid, closed } = z
        .object({ thread: z.string(), closed: z.boolean() })
        .parse(request);
      const thread = threads.find((t) => t.id === tid);
      if (!thread) throw new Error('Unknown screenshot thread');
      thread.closed = closed;
      thread.closed_by = closed ? 'vladko' : null;
      thread.closed_at = closed ? now : null;
      snapshot.threads = snapshot.threads.map((t) =>
        t.id === tid
          ? {
              ...t,
              closed: thread.closed,
              closed_by: thread.closed_by,
              closed_at: thread.closed_at,
            }
          : t,
      );
      return null;
    }
    if (request.kind === 'delete') {
      const { thread: tid } = z.object({ thread: z.string() }).parse(request);
      snapshot.threads = snapshot.threads.filter((t) => t.id !== tid);
      return null;
    }
  }
  throw new Error(
    `Unexpected screenshot IPC: ${command} ${JSON.stringify(payload)}`,
  );
});

await import('../src/main');
