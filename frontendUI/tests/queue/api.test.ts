import { test } from 'node:test';
import assert from 'node:assert/strict';
import { getQueue, submitEntry, moveEntry, setVotingPolicy, advance, veto, vote, QueueError, newRequestId } from '../../app/lib/queue.ts';
import { queue } from './support.ts';

test('queue transport uses bearer auth, exact mutation bodies and stable error codes', async () => {
  const originalFetch = globalThis.fetch;
  const originalNavigator = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
  const requests: { path: string; init: RequestInit }[] = [];
  let conflict = false;
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { locks: { request: (_name: string, fn: () => unknown) => fn() } } });
  globalThis.fetch = async (input, init = {}) => {
    const path = String(input);
    if (path === '/api/auth/refresh') return Response.json({ access_token: 'opaque-test-token' });
    if (path === '/api/auth/guest') return Response.json({ returning: false, last_nickname: null, session: true });
    requests.push({ path, init });
    if (conflict) return Response.json({ code: 'stale_queue_revision', error: 'Refresh the queue.' }, { status: 409 });
    return Response.json(path.endsWith('/queue') ? queue() : { changed: true, queue: queue() });
  };
  try {
    const member = 'a'.repeat(32), id = newRequestId(); assert.match(id, /^[0-9a-f]{32}$/);
    await getQueue(member); await submitEntry(member, 'https://youtu.be/dQw4w9WgXcQ', id);
    await moveEntry(member, id, '9007199254740993', 0);
    await setVotingPolicy(member, { mode: 'fixed', count: 2147483647 });
    await advance(member, id, 'playback_error_skip'); await veto(member, id); await vote(member, id);
    assert.deepEqual(JSON.parse(requests[1].init.body as string), { url: 'https://youtu.be/dQw4w9WgXcQ', request_id: id });
    assert.deepEqual(JSON.parse(requests[2].init.body as string), { expected_revision: '9007199254740993', to_index: 0 });
    assert.equal(requests[3].init.method, 'PUT');
    assert.equal(requests[5].init.method, 'DELETE'); assert.equal(requests[5].init.body, undefined);
    assert.equal(requests[6].init.method, 'POST'); assert.equal(requests[6].init.body, undefined);
    for (const request of requests) {
      assert.equal(new Headers(request.init.headers).get('Authorization'), 'Bearer opaque-test-token');
      assert.equal(request.init.cache, 'no-store');
      assert.ok(request.path.startsWith(`/api/lobbies/memberships/${member}/queue`));
    }
    conflict = true;
    await assert.rejects(moveEntry(member, id, '1', 0), e => e instanceof QueueError && e.status === 409 && e.code === 'stale_queue_revision');
  } finally {
    globalThis.fetch = originalFetch;
    if (originalNavigator) Object.defineProperty(globalThis, 'navigator', originalNavigator);
  }
});
