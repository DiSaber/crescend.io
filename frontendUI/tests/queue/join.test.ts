import { test } from 'node:test';
import assert from 'node:assert/strict';
import { joinLobby, getCurrent } from '../../app/lib/lobby.ts';
import { current } from './support.ts';

test('main lobby transfer behavior survives auth/recovery integration', async () => {
  const saved = { fetch: globalThis.fetch, navigator: Object.getOwnPropertyDescriptor(globalThis, 'navigator'), window: Object.getOwnPropertyDescriptor(globalThis, 'window') };
  let mode: 'guest' | 'host' | 'cancel' = 'guest';
  let confirmation = 0;
  const requests: { path: string; method: string }[] = [];
  const old = current(); const replacement = current('b'.repeat(32));
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { locks: { request: (_name: string, fn: () => unknown) => fn() } } });
  globalThis.fetch = async (input, init = {}) => {
    const path = String(input), method = init.method ?? 'GET';
    if (path === '/api/auth/refresh') return Response.json({ access_token: 'test-token' });
    requests.push({ path, method });
    if (path.endsWith('/current')) return Response.json(old);
    if (method === 'DELETE') return new Response(null, { status: 204 });
    if (path.endsWith('/join')) {
      const deletes = requests.filter(r => r.method === 'DELETE').length;
      if (mode === 'guest' || deletes) return Response.json(replacement);
      return Response.json({ code: 'already_in_lobby', error: 'Host already in a lobby.' }, { status: 409 });
    }
    throw new Error(`Unexpected request ${path}`);
  };
  try {
    // Bootstrap before introducing a fake Window so no real BroadcastChannel is opened.
    await getCurrent(); requests.length = 0;
    Object.defineProperty(globalThis, 'window', { configurable: true, value: { confirm: () => { confirmation++; return mode !== 'cancel'; } } });
    assert.deepEqual(await joinLobby('NEW123'), replacement);
    assert.equal(confirmation, 0); assert.equal(requests.length, 1);
    requests.length = 0; mode = 'host';
    assert.deepEqual(await joinLobby('NEW123'), replacement);
    assert.equal(confirmation, 1);
    assert.deepEqual(requests.map(r => r.method), ['POST', 'GET', 'DELETE', 'POST']);
    assert.equal(requests[2].path, `/api/lobbies/memberships/${old.membership_id}`);
    requests.length = 0; mode = 'cancel';
    await assert.rejects(joinLobby('NEW123'), /still active/);
    assert.equal(requests.some(r => r.method === 'DELETE'), false);
  } finally {
    globalThis.fetch = saved.fetch;
    for (const key of ['navigator', 'window'] as const) {
      const descriptor = saved[key];
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
});
