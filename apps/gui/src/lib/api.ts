import { invoke } from '@tauri-apps/api/core';
import { z } from 'zod';

// These are the public DTOs in src-tauri/src/bridge.rs. Private device/channel keys and device bearer
// tokens never cross this boundary; malformed native responses fail closed.
export const id = z.string().regex(/^[A-Za-z0-9_-]{22}$/);
const integer = z.number().int().nonnegative().safe();
const kind = z.enum(['cli', 'gui']);
export const reactionKind = z.enum(['up', 'down', 'ack', 'claim']);
const summaryMode = z.enum(['required', 'off']);
export const profileSchema = z.object({
  id: z.string().regex(/^[a-f0-9]{64}$/),
  name: z.string(),
  server: z.url(),
  space_id: id,
  person_id: id,
  device_id: id,
  display_name: z.string(),
  summarizer_configured: z.boolean(),
});
export const reactionSchema = z.object({
  kind: reactionKind,
  person_id: id,
  author: z.string(),
  handle: z.string(),
  created_at: integer,
});
export const reactionSummarySchema = z.object({
  up: integer,
  down: integer,
  ack: integer,
  claims: z.array(
    z.object({
      message: id,
      person: id,
      author: z.string(),
      handle: z.string(),
    }),
  ),
});
export type ReactionSummary = z.infer<typeof reactionSummarySchema>;
export const entrySchema = z.object({
  id,
  channel: z.string(),
  to: z.string(),
  title: z.string(),
  from: z.string(),
  created_at: integer,
  summary: z.string().nullable(),
  mentioned: z.boolean(),
  unread: z.boolean(),
  own_agent_activity: z.boolean(),
  latest_seq: integer,
  reactions: z.array(reactionSchema),
  reaction_summary: reactionSummarySchema,
});
export const snapshotSchema = z.object({
  profile: profileSchema,
  channels: z.array(
    z.object({
      id,
      name: z.string(),
      kind: z.enum(['all', 'group', 'dm', 'self']),
    }),
  ),
  threads: z.array(entrySchema),
  retention_days: integer.max(4294967295),
  summary_mode: summaryMode,
  members: z.record(id, z.string()),
  devices: z.array(
    z.object({
      id,
      person_id: id,
      kind,
      revoked_at: integer.nullable(),
      last_seen_at: integer.nullable(),
    }),
  ),
});
const preview = z
  .string()
  .regex(/^data:image\/(png|jpeg|gif|webp);base64,[A-Za-z0-9+/]*={0,2}$/)
  .nullable();
export const threadSchema = z.object({
  id,
  title: z.string(),
  participants: z.array(z.string()),
  summary: z.string().nullable(),
  summary_number: integer.nullable(),
  newer_messages: integer,
  messages: z.array(
    z.object({
      id,
      number: integer,
      author: z.string(),
      person_id: id,
      handle: z.string(),
      kind,
      created_at: integer,
      body: z.string(),
      verification_error: z.string().nullable(),
      reactions: z.array(reactionSchema),
      reaction_summary: reactionSummarySchema,
      attachments: z.array(
        z.object({
          name: z.string(),
          mime: z.string(),
          size: integer,
          path: z.string(),
          preview,
        }),
      ),
    }),
  ),
});
export const pollSchema = z.object({
  changed: z.boolean(),
  errors: z.array(z.string()),
  notices: z.array(z.object({ mentioned: z.boolean(), direct: z.boolean() })),
});
const target = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('device'), device_id: id }),
  z.object({ kind: z.literal('person'), person_id: id }),
]);
const requestSchema = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('snapshot') }),
  z.object({ kind: z.literal('read'), thread: id }),
  z.object({
    kind: z.literal('send'),
    thread: id.nullable(),
    to: z.string(),
    title: z.string(),
    body: z
      .string()
      .min(1)
      .max(256 * 1024),
  }),
  z.object({
    kind: z.literal('react'),
    message: id,
    reaction: reactionKind,
    remove: z.boolean(),
  }),
  z.object({ kind: z.literal('unread'), thread: id }),
  z.object({
    kind: z.literal('settings'),
    retention_days: integer.max(4294967295),
    summary_mode: summaryMode,
  }),
  z.object({
    kind: z.literal('display_name'),
    name: z.string().trim().min(1).max(120),
  }),
  z.object({ kind: z.literal('revoke'), target }),
  z.object({
    kind: z.literal('grant'),
    enroll: z.boolean(),
    recovery: z.boolean(),
    device_kind: kind,
  }),
]);
export type Profile = z.infer<typeof profileSchema>;
export type Snapshot = z.infer<typeof snapshotSchema>;
export type Thread = z.infer<typeof threadSchema>;
export type Entry = z.infer<typeof entrySchema>;
export type Reaction = z.infer<typeof reactionSchema>;
export type ReactionKind = z.infer<typeof reactionKind>;
export type Request = z.infer<typeof requestSchema>;
export type Poll = z.infer<typeof pollSchema>;
export async function rpc<T>(
  profile: string,
  request: Request,
  schema: z.ZodType<T>,
): Promise<T> {
  return schema.parse(
    await invoke<unknown>('execute', {
      profile,
      request: requestSchema.parse(request),
    }),
  );
}
export async function profiles(): Promise<Profile[]> {
  return z.array(profileSchema).parse(await invoke<unknown>('profiles'));
}
export type Provision =
  | { kind: 'join'; link: string; display_name: string }
  | {
      kind: 'create';
      server: string;
      name: string;
      display_name: string;
      admin_token: string | null;
    };
export async function provision(request: Provision): Promise<Profile> {
  return profileSchema.parse(await invoke<unknown>('provision', { request }));
}
export async function poll(profile: string, initial: boolean): Promise<Poll> {
  return pollSchema.parse(await invoke<unknown>('poll', { profile, initial }));
}
export const nothing = z.null();
export const grantLink = z
  .string()
  .regex(/^phis?:\/\/[^\s]+#[A-Za-z0-9_-]{43}$/);
