import { test } from 'node:test';
import assert from 'node:assert/strict';
import { AuthCoordinator, SignedOutError } from '../../app/lib/auth-coordinator.ts';

function deferred<T>() { let resolve!: (v: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { promise, resolve }; }
function setup() {
  let calls = 0; const notices: string[] = [];
  const env = { fetch: async () => { calls++; return Response.json({ access_token: 'token' }); }, lock: async <T>(fn: () => Promise<T>) => fn(), publish: (reason: string) => { notices.push(reason); } };
  return { env, calls: () => calls, notices };
}
test('uncertain refresh stops automatic retries', async () => {
  const f = setup(); f.env.fetch = async () => { throw new TypeError('network'); };
  const auth = new AuthCoordinator(f.env);
  await assert.rejects(auth.refresh());
  assert.equal(await auth.refresh(), undefined); assert.equal(f.notices.length, 1);
});
test('logout intent rejects late token restoration', async () => {
  const f = setup(); const response = deferred<Response>(); f.env.fetch = () => response.promise;
  const auth = new AuthCoordinator(f.env); const pending = auth.refresh();
  auth.stop('signed-out'); response.resolve(Response.json({ access_token: 'late' }));
  assert.equal(await pending, undefined); assert.equal(auth.token(), undefined);
});
test('queued refresh rechecks stop after acquiring lock', async () => {
  const f = setup(); const lock = deferred<void>();
  f.env.lock = async fn => { await lock.promise; return fn(); };
  const auth = new AuthCoordinator(f.env); const pending = auth.refresh();
  auth.stop('signed-out'); lock.resolve(); await pending;
  assert.equal(f.calls(), 0);
});
test('logout reports failure unless server confirms 204', async () => {
  const f = setup(); f.env.fetch = async () => new Response(null, { status: 500 });
  const auth = new AuthCoordinator(f.env);
  await assert.rejects(auth.logout(), /Logout failed/);
  assert.equal(auth.token(), undefined);
});
test('protected requests cannot complete after remote logout', async () => {
  const f = setup(); const auth = new AuthCoordinator(f.env); await auth.refresh();
  const response = deferred<Response>(); f.env.fetch = () => response.promise;
  const pending = auth.apiFetch('/protected'); auth.stop('signed-out');
  response.resolve(Response.json({ secret: true })); await assert.rejects(pending, SignedOutError);
});
