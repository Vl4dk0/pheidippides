import { z } from 'zod';
import type { Entry, Poll, Reaction, ReactionKind } from './api';
export type Filter = 'all' | 'unread' | 'agents' | 'mentions';
export function filterThreads(
  threads: Entry[],
  filter: Filter,
  channel: string,
): Entry[] {
  return threads
    .filter(
      (t) =>
        (!channel || t.channel === channel) &&
        (filter === 'all' ||
          (filter === 'unread' && t.unread) ||
          (filter === 'agents' && t.own_agent_activity) ||
          (filter === 'mentions' && t.mentioned)),
    )
    .sort((a, b) => b.latest_seq - a.latest_seq);
}
export function reactionLabel(reactions: Reaction[]): string {
  const counts = new Map<ReactionKind, number>();
  for (const reaction of reactions)
    counts.set(reaction.kind, (counts.get(reaction.kind) ?? 0) + 1);
  return [...counts]
    .map(([kind, count]) =>
      kind === 'claim'
        ? `claimed by ${reactions.find((r) => r.kind === 'claim')?.author}/${reactions.find((r) => r.kind === 'claim')?.handle}`
        : `${kind} × ${count}`,
    )
    .join(' · ');
}
export function shouldNotify(event: Poll, initial: boolean): boolean {
  return (
    !initial &&
    event.notices.some((notice) => notice.mentioned || notice.direct)
  );
}
export const preferencesSchema = z.object({
  spaces: z.boolean(),
  threads: z.boolean(),
  view: z.boolean(),
  summary: z.boolean(),
  reactions: z.boolean(),
  attachments: z.boolean(),
  metadata: z.boolean(),
  notifications: z.boolean(),
  theme: z.enum(['system', 'light', 'dark']),
});
export type Preferences = z.infer<typeof preferencesSchema>;
export const defaultPreferences: Preferences = {
  spaces: true,
  threads: true,
  view: true,
  summary: true,
  reactions: true,
  attachments: true,
  metadata: true,
  notifications: false,
  theme: 'system',
};
export function parsePreferences(raw: string | null): Preferences {
  try {
    const parsed = preferencesSchema.safeParse(JSON.parse(raw ?? 'null'));
    return parsed.success ? parsed.data : { ...defaultPreferences };
  } catch {
    return { ...defaultPreferences };
  }
}
// A response belongs to the selection that requested it, even after rapid navigation.
export class Selection {
  private revision = 0;
  change(): number {
    return ++this.revision;
  }
  current(revision: number): boolean {
    return revision === this.revision;
  }
}
