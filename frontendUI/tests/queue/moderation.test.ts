import { test } from 'node:test';
import assert from 'node:assert/strict';
import { QueueSession } from '../../app/lib/queue-state.ts';
import { fixture, entry, queue, current } from './support.ts';

test('vote snapshots reflect own vote and never invent a voter roster', async () => {
  const f = fixture(); f.deps.identity = async () => '2';
  const voted = { ...entry(), has_voted: true, active_vote_count: 1 };
  f.deps.vote = async () => ({ changed: false, queue: queue('2', [voted]) });
  const s = new QueueSession(f.deps); s.start(); await s.sync(); await s.vote(entry().id);
  assert.equal(s.getSnapshot().queue?.current?.has_voted, true);
  assert.equal(s.getSnapshot().queue?.current?.active_vote_count, 1);
  assert.equal('voters' in s.getSnapshot().queue!.current!, false); s.stop();
});
test('leaving/rejoining takes retained votes and policy only from the new generation snapshot', async () => {
  const f = fixture(); const s = new QueueSession(f.deps); s.start(); await s.sync();
  const old = s.capture();
  f.deps.current = async () => current('b'.repeat(32));
  const next = queue('3', [{ ...entry(), has_voted: true, active_vote_count: 1 }], 'b'.repeat(32));
  f.deps.read = async () => next; await s.sync();
  s.applyQueue(queue('999', [entry()]), old);
  assert.equal(s.getSnapshot().queue?.current?.has_voted, true); assert.equal(s.getSnapshot().queue?.revision, '3'); s.stop();
});
test('zero-guest null threshold and fixed policy come from server, not local vote arithmetic', async () => {
  const f = fixture(); const view = queue('2', [entry()]);
  view.voting = { mode: 'fixed', fixed_count: 2147483647, active_guest_count: 0, effective_threshold: null };
  f.deps.policy = async () => ({ changed: true, queue: view });
  const s = new QueueSession(f.deps); s.start(); await s.sync(); await s.policy({ mode: 'fixed', count: 2147483647 });
  assert.equal(s.getSnapshot().queue?.voting.effective_threshold, null);
  assert.ok(s.getSnapshot().queue?.current); s.stop();
});
test('invalid fixed thresholds are rejected before a request', async () => {
  const f = fixture(); let calls = 0;
  f.deps.policy = async () => { calls++; return { changed: true, queue: queue() }; };
  const s = new QueueSession(f.deps); s.start(); await s.sync();
  for (const count of [0, -1, 1.5, NaN, 2147483648]) await assert.rejects(s.policy({ mode: 'fixed', count }));
  assert.equal(calls, 0); s.stop();
});
