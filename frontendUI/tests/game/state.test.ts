import { test } from 'node:test';
import assert from 'node:assert/strict';
import { QueueSession } from '../../app/lib/queue-state.ts';
import type { GameSnapshot } from '../../app/lib/game.ts';
import { fixture } from '../queue/support.ts';

const snapshot = (revision: string, serverTime = new Date().toISOString()): GameSnapshot => ({
  membership_id: 'a'.repeat(32), lobby_id: 'c'.repeat(32), revision, server_time: serverTime, game: null,
});

test('sync reads the game alongside the queue', async () => {
  const f = fixture(); let reads = 0;
  f.deps.readGame = async () => { reads++; return snapshot('1'); };
  const session = new QueueSession(f.deps); session.start(); await session.sync();
  assert.ok(reads >= 1);
  assert.equal(session.getSnapshot().game?.revision, '1'); session.stop();
});

test('older game snapshots never replace newer ones', async () => {
  const f = fixture(); const session = new QueueSession(f.deps); session.start(); await session.sync();
  const epoch = session.capture();
  session.applyGame(snapshot('10'), epoch);
  session.applyGame(snapshot('9'), epoch);
  assert.equal(session.getSnapshot().game?.revision, '10');
  session.applyGame({ ...snapshot('11'), lobby_id: 'd'.repeat(32) }, epoch);
  assert.equal(session.getSnapshot().game?.revision, '10'); session.stop();
});

test('clock offset follows server_time, not the device clock', async () => {
  const f = fixture(); const session = new QueueSession(f.deps); session.start(); await session.sync();
  session.applyGame(snapshot('2', new Date(Date.now() + 120000).toISOString()), session.capture());
  const offset = session.getSnapshot().clockOffset;
  assert.ok(Math.abs(offset - 120000) < 1000, `offset ${offset}`); session.stop();
});

test('routine game conflicts resync quietly', async () => {
  const f = fixture(); const session = new QueueSession(f.deps); session.start(); await session.sync();
  const { QueueError } = await import('../../app/lib/queue.ts');
  await assert.rejects(session.gameMutate('Skipping…', async () => { throw new QueueError(409, 'game_phase_changed', 'moved on'); }));
  assert.equal(session.getSnapshot().gameError, null);
  await assert.rejects(session.gameMutate('Voting…', async () => { throw new QueueError(409, 'vote_limit_reached', 'No votes left'); }));
  assert.equal(session.getSnapshot().gameError, 'No votes left'); session.stop();
});
