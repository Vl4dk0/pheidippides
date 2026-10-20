import { z } from 'zod';
import type { Entry, Poll } from './api';
export type Filter = 'all' | 'unread' | 'agents' | 'mentions' | 'closed';
export function filterThreads(
  threads: Entry[],
  filter: Filter,
  channel: string,
): Entry[] {
  return threads
    .filter(
      (t) =>
        (!channel || t.channel === channel) &&
        (filter === 'closed'
          ? t.closed
          : !t.closed &&
            (filter === 'all' ||
              (filter === 'unread' && t.unread) ||
              (filter === 'agents' && t.own_agent_activity) ||
              (filter === 'mentions' && t.mentioned))),
    )
    .sort((a, b) => b.latest_seq - a.latest_seq);
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
