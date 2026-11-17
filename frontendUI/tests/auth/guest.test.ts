import { test } from 'node:test';
import assert from 'node:assert/strict';
import { AuthCoordinator, SignedOutError } from '../../app/lib/auth-coordinator.ts';
import { nicknameProblem } from '../../app/lib/lobby.ts';

function setup(respond: (path: string) => Response) {
  const paths: string[] = [];
  const env = {
    fetch: async (input: RequestInfo | URL) => { paths.push(String(input)); return respond(String(input)); },
    lock: async <T>(fn: () => Promise<T>) => fn(),
    publish: () => {},
  };
  return { env, paths };
}

test('guest sign-in restarts a signed-out coordinator and serves the new token', async () => {
  const f = setup(path => path === '/api/auth/refresh'
    ? new Response(null, { status: 401 })
    : Response.json({ access_token: 'guest-token', token_type: 'Bearer', expires_in: 3600 }));
  const auth = new AuthCoordinator(f.env);
  assert.equal(await auth.getToken(), undefined, 'no cookie yet: signed out');
  assert.equal(await auth.guestSignIn(), 'guest-token');
  assert.equal(await auth.getToken(), 'guest-token');
  assert.deepEqual(f.paths, ['/api/auth/refresh', '/api/auth/guest']);
});

test('guest sign-in reports the rate limit and leaves no token', async () => {
  const f = setup(() => Response.json({ code: 'too_many_guests' }, { status: 429 }));
  const auth = new AuthCoordinator(f.env);
  await assert.rejects(auth.guestSignIn(), /Too many new guests/);
  assert.equal(auth.token(), undefined);
});

test('a logout during guest sign-in wins', async () => {
  let release!: () => void;
  const gate = new Promise<void>(r => { release = r; });
  const f = setup(() => Response.json({ access_token: 'late' }));
  f.env.lock = async <T>(fn: () => Promise<T>) => { await gate; return fn(); };
  const auth = new AuthCoordinator(f.env);
  const pending = auth.guestSignIn();
  auth.stop('signed-out');
  release();
  await assert.rejects(pending, SignedOutError);
  assert.equal(auth.token(), undefined);
});

test('nickname rules match the server', () => {
  assert.equal(nicknameProblem('  Sam  '), undefined);
  assert.equal(nicknameProblem('<svg onload=alert(1)>'), undefined, 'markup is just text');
  assert.equal(nicknameProblem('x'.repeat(24)), undefined);
  assert.equal(nicknameProblem('é'.repeat(24)), undefined, 'characters, not bytes');
  assert.match(nicknameProblem('   ') ?? '', /Pick/);
  assert.match(nicknameProblem('x'.repeat(25)) ?? '', /at most 24/);
  assert.match(nicknameProblem('a\u0007b') ?? '', /control/);
  assert.match(nicknameProblem('rtl‮x') ?? '', /control/);
});
