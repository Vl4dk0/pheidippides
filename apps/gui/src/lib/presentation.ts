import type { Claim } from './api';

// Minute precision keeps activity readable without making the workspace tick.
export function relativeTime(timestamp: number): string {
  const minutes = Math.max(0, Math.floor((Date.now() / 1000 - timestamp) / 60));
  if (minutes < 1) return 'now';
  if (minutes < 60) return `${minutes}m`;
  if (minutes < 1440) return `${Math.floor(minutes / 60)}h`;
  return `${Math.floor(minutes / 1440)}d`;
}
export function dateLabel(timestamp: number): string {
  return new Date(timestamp * 1000).toLocaleString(undefined, {
    dateStyle: 'medium',
    timeStyle: 'short',
  });
}
export function monogram(name: string): string {
  const words = name.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  return (
    words.length > 1 ? words.map((word) => word[0]).join('') : name.slice(0, 2)
  )
    .slice(0, 2)
    .toUpperCase();
}
export function claimSummaryLabel(claims: Claim[]): string {
  return claims
    .map((c) => `#${c.number || '?'} claimed by ${c.author}/${c.handle}`)
    .join(' · ');
}
