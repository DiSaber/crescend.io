import type { CurrentLobby } from '../../app/lib/lobby.ts';
import type { QueueView, QueueEntry } from '../../app/lib/queue.ts';
import type { SessionDependencies } from '../../app/lib/queue-state.ts';
export function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { promise, resolve }; }
export const entry = (id = 'a'): QueueEntry => ({
  id: id.repeat(32), video: { video_id: 'dQw4w9WgXcQ', title: 'Song', author_name: 'Artist', author_url: '', thumbnail_url: '', embed_url: '' },
  submitted_by_user_id: '1', submitted_at: '2026-10-29T00:00:00Z', active_vote_count: 0, has_voted: false,
});
export function current(id = 'a'.repeat(32)): CurrentLobby {
  return { membership_id: id, lobby: { id: 'c'.repeat(32), join_code: 'ABC123', owner_user_id: '1', created_at: '', expires_at: '', revision: '1', members: [] } };
}
export function queue(revision = '1', entries: QueueEntry[] = [], membership = 'a'.repeat(32)): QueueView {
  return { membership_id: membership, lobby_id: 'c'.repeat(32), revision, current: entries[0] ?? null, upcoming: entries.slice(1), voting: { mode: 'majority', fixed_count: null, active_guest_count: 2, effective_threshold: 2 } };
}
export function fixture() {
  let reads = 0;
  const response = async () => ({ changed: true, queue: queue('2') });
  const deps: SessionDependencies = {
    current: async () => current(), read: async () => { reads++; return queue(); }, identity: async () => '1',
    events: async (_m, signal) => new Promise<void>(resolve => signal.addEventListener('abort', () => resolve(), { once: true })),
    refresh: async () => 'token', submit: async () => ({ changed: true, entry_id: 'a'.repeat(32), active: true, queue: queue('2') }),
    advance: response, veto: response, move: response, vote: response, policy: response, leave: async () => {},
  };
  return { deps, queueReads: () => reads };
}
