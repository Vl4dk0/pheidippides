import { describe, expect, it } from 'vitest';
import {
  entrySchema,
  pollSchema,
  threadSchema,
  reactionSummarySchema,
  snapshotSchema,
  type Entry,
} from './api';
import {
  defaultPreferences,
  filterThreads,
  parsePreferences,
  Selection,
  shouldNotify,
} from './state';
const id = 'AAAAAAAAAAAAAAAAAAAAAA';
const entry: Entry = {
  id,
  channel: '#all',
  to: '#all',
  title: 'Work',
  from: 'vladko/main',
  created_at: 1,
  summary: null,
  mentioned: false,
  unread: false,
  own_agent_activity: false,
  latest_seq: 1,
  reactions: [],
  reaction_summary: { up: 0, down: 0, ack: 0, claims: [] },
};
describe('desktop state boundary', () => {
  it('filters channel, unread, agent activity and mentions independently, sorted by sequence', () => {
    const items = [
      entry,
      {
        ...entry,
        id: 'BBBBBBBBBBBBBBBBBBBBBB',
        unread: true,
        mentioned: true,
        own_agent_activity: true,
        latest_seq: 5,
      },
      { ...entry, channel: '@bob', latest_seq: 3 },
    ];
    expect(filterThreads(items, 'all', '').map((t) => t.latest_seq)).toEqual([
      5, 3, 1,
    ]);
    for (const filter of ['unread', 'agents', 'mentions'] as const)
      expect(filterThreads(items, filter, '#all')).toEqual([items[1]]);
    expect(filterThreads(items, 'all', '@bob')).toEqual([items[2]]);
    expect(items[0]).toBe(entry);
  });
  it('rejects malformed native IDs, unsafe sequences and active image content', () => {
    expect(entrySchema.safeParse({ ...entry, id: '../keys' }).success).toBe(
      false,
    );
    expect(
      entrySchema.safeParse({
        ...entry,
        latest_seq: Number.MAX_SAFE_INTEGER + 1,
      }).success,
    ).toBe(false);
    expect(
      pollSchema.safeParse({
        changed: true,
        errors: [],
        notices: [{ direct: 'yes' }],
      }).success,
    ).toBe(false);
    const message = {
      id,
      number: 1,
      author: 'a',
      person_id: id,
      handle: 'gui',
      kind: 'gui',
      created_at: 1,
      body: '',
      verification_error: null,
      reactions: [],
      reaction_summary: { up: 0, down: 0, ack: 0, claims: [] },
      attachments: [
        {
          name: 'x.svg',
          mime: 'image/svg+xml',
          size: 1,
          path: '/cache/x.svg',
          preview: 'data:image/svg+xml;base64,PHN2Zz4=',
        },
      ],
    };
    expect(
      threadSchema.safeParse({
        id,
        title: '',
        participants: [],
        summary: null,
        summary_number: null,
        newer_messages: 0,
        messages: [message],
      }).success,
    ).toBe(false);
    const interval = snapshotSchema.shape.summary_every;
    expect(interval.parse(3)).toBe(3);
    for (const bad of [0, -1, 1.5, 4294967296]) {
      expect(interval.safeParse(bad).success).toBe(false);
    }
    expect(entrySchema.parse(entry)).toEqual(entry);
  });
  it('keeps the initial event backlog quiet and notifies only on mentions or DMs', () => {
    const event = {
      changed: true,
      errors: [],
      notices: [{ mentioned: true, direct: false }],
    };
    expect(shouldNotify(event, true)).toBe(false);
    expect(shouldNotify(event, false)).toBe(true);
    expect(
      shouldNotify(
        {
          changed: true,
          errors: [],
          notices: [{ mentioned: false, direct: true }],
        },
        false,
      ),
    ).toBe(true);
    expect(
      shouldNotify({ changed: true, errors: [], notices: [] }, false),
    ).toBe(false);
  });
  it('restores validated preferences and recovers from corrupt or outdated storage', () => {
    for (const raw of [
      null,
      '{',
      '{}',
      JSON.stringify({ ...defaultPreferences, spaces: 'invalid' }),
    ])
      expect(parsePreferences(raw)).toEqual(defaultPreferences);
    expect(
      parsePreferences(JSON.stringify({ ...defaultPreferences, spaces: false }))
        .spaces,
    ).toBe(false);
  });
  it('accepts structured reaction context including multiple claimants', () => {
    const summary = {
      up: 0,
      down: 0,
      ack: 2,
      claims: [
        { message: id, person: id, author: 'bob', handle: 'fixer' },
        {
          message: 'BBBBBBBBBBBBBBBBBBBBBB',
          person: id,
          author: 'bob',
          handle: 'reviewer',
        },
      ],
    };
    expect(reactionSummarySchema.parse(summary).claims).toHaveLength(2);
    expect(
      reactionSummarySchema.safeParse({ ...summary, ack: -1 }).success,
    ).toBe(false);
  });
  it('discards stale selections after switching spaces or threads', () => {
    const selection = new Selection();
    const old = selection.change();
    const current = selection.change();
    expect(selection.current(old)).toBe(false);
    expect(selection.current(current)).toBe(true);
  });
});
