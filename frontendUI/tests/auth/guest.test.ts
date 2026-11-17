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

test('emoji built with joiners and selectors count as one character each', async () => {
  const { nicknameLength, normalizeCode } = await import('../../app/lib/lobby.ts');
  const family = '👨‍👩‍👧‍👦';
  const rainbow = '🏳️‍🌈';
  for (const name of [family, `${rainbow} Kat`, 'Kat ❤️', '🇺🇸🇯🇵', family.repeat(24)]) {
    assert.equal(nicknameProblem(name), undefined, name);
  }
  assert.equal(nicknameLength(family.repeat(7)), 7);
  assert.match(nicknameProblem(family.repeat(25)) ?? '', /at most 24/);
  for (const bad of ['​', 'Ann​ie', 'Kat‍', 'A‍nn', 'Kat️', 'iso⁦x']) {
    assert.match(nicknameProblem(bad) ?? '', /invisible/, JSON.stringify(bad));
  }
  assert.equal(normalizeCode(' abc 123 '), 'ABC123');
  assert.equal(normalizeCode('ABC-123'), 'ABC123');
  assert.equal(normalizeCode('https://crescend.io/join/?code=abc123'), 'ABC123');
  assert.equal(normalizeCode('A.B,C!1@2#3$4'), 'ABC123');
});

test('a browser with no session cookie skips the refresh that would 401', async () => {
  const f = setup(() => { throw new Error('no request expected'); });
  let asked = 0;
  const auth = new AuthCoordinator({ ...f.env, hasSession: async () => { asked++; return false; } });
  assert.equal(await auth.getToken(), undefined);
  assert.deepEqual(f.paths, [], 'no refresh request');
  assert.equal(asked, 1);
});

test('a session cookie still refreshes, and a wrong code stops guest sign-in', async () => {
  const f = setup(path => path === '/api/auth/refresh'
    ? Response.json({ access_token: 'tok', token_type: 'Bearer', expires_in: 3600 })
    : Response.json({ code: 'lobby_unavailable' }, { status: 404 }));
  const auth = new AuthCoordinator({ ...f.env, hasSession: async () => true });
  assert.equal(await auth.getToken(), 'tok');
  const fresh = new AuthCoordinator(f.env);
  await assert.rejects(fresh.guestSignIn('ZZZZZZ'), /No active session has that code/);
  assert.deepEqual(f.paths, ['/api/auth/refresh', '/api/auth/guest?join_code=ZZZZZZ']);
});
