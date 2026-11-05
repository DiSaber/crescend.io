import { test } from 'node:test';
import assert from 'node:assert/strict';
import { QueueSession } from '../../app/lib/queue-state.ts';
import { QueueError } from '../../app/lib/queue.ts';
import { fixture, entry, queue, deferred } from './support.ts';

test('five old callbacks cannot advance a replacement occurrence of the same video', async () => {
  const f = fixture(); let calls = 0;
  f.deps.read = async () => queue('2', [entry('b')]);
  f.deps.advance = async () => { calls++; return { changed: true, queue: queue('3') }; };
  const s = new QueueSession(f.deps); s.start(); await s.sync();
  for (let i = 0; i < 5; i++) await s.advanceEntry(entry('a').id, 'ended');
  assert.equal(calls, 0); assert.equal(s.getSnapshot().queue?.current?.id, entry('b').id); s.stop();
});
test('concurrent end and skip send only one progression request', async () => {
  const f = fixture(); let calls = 0; const result = deferred<{ changed: boolean; queue: ReturnType<typeof queue> }>();
  f.deps.read = async () => queue('1', [entry()]);
  f.deps.advance = async () => { calls++; return result.promise; };
  const s = new QueueSession(f.deps); s.start(); await s.sync();
  const first = s.advanceEntry(entry().id, 'ended');
  await s.advanceEntry(entry().id, 'skip');
  result.resolve({ changed: true, queue: queue('2', [entry('b')]) }); await first;
  assert.equal(calls, 1); assert.equal(s.getSnapshot().queue?.current?.id, entry('b').id); s.stop();
});
test('stale reorder recovers authoritative order without replaying the move', async () => {
  const f = fixture(); let calls = 0;
  f.deps.move = async () => { calls++; throw new QueueError(409, 'stale_queue_revision', 'Queue changed. Choose a new move.'); };
  const s = new QueueSession(f.deps); s.start(); await s.sync();
  f.deps.read = async () => queue('8', [entry(), entry('b')]);
  await assert.rejects(s.move(entry('b').id, '1', 0), /Queue changed/);
  assert.equal(calls, 1); assert.equal(s.getSnapshot().queue?.revision, '8');
  assert.equal(s.getSnapshot().busy, null); s.stop();
});
test('guest cannot call host mutation paths; host cannot cast guest votes', async () => {
  const f = fixture(); let calls = 0; f.deps.identity = async () => '2';
  f.deps.read = async () => queue('1', [entry()]);
  const op = async () => { calls++; return { changed: true, queue: queue('2') }; };
  f.deps.advance = op; f.deps.veto = op; f.deps.move = op; f.deps.policy = op; f.deps.vote = op;
  const guest = new QueueSession(f.deps); guest.start(); await guest.sync();
  await guest.advanceEntry(entry().id, 'skip'); await guest.remove(entry().id);
  await guest.move(entry().id, '1', 0); await guest.policy({ mode: 'majority' });
  assert.equal(calls, 0); guest.stop();
  f.deps.identity = async () => '1';
  const host = new QueueSession(f.deps); host.start(); await host.sync(); await host.vote(entry().id);
  assert.equal(calls, 0); host.stop();
});
test('uncertain advance refetches but never automatically substitutes the replacement ID', async () => {
  const f = fixture(); let calls = 0;
  f.deps.read = async () => queue('1', [entry()]);
  f.deps.advance = async () => { calls++; f.deps.read = async () => queue('2', [entry('b')]); throw new TypeError('response lost'); };
  const s = new QueueSession(f.deps); s.start(); await s.sync();
  await assert.rejects(s.advanceEntry(entry().id, 'skip'));
  assert.equal(calls, 1); assert.equal(s.getSnapshot().queue?.current?.id, entry('b').id); s.stop();
});
test('natural end during a submission is delivered after it settles, with captured occurrence', async () => {
  const f = fixture(); let advances = 0;
  const submission = deferred<{ changed: boolean; entry_id: string; active: boolean; queue: ReturnType<typeof queue> }>();
  f.deps.read = async () => queue('1', [entry()]);
  f.deps.submit = () => submission.promise;
  f.deps.advance = async (_m, id) => { assert.equal(id, entry().id); advances++; return { changed: true, queue: queue('3') }; };
  const s = new QueueSession(f.deps); s.start(); await s.sync();
  const pending = s.add('https://youtu.be/dQw4w9WgXcQ');
  await s.advanceEntry(entry().id, 'ended');
  submission.resolve({ changed: true, entry_id: entry('b').id, active: true, queue: queue('2', [entry(), entry('b')]) });
  await pending; await new Promise(resolve => setImmediate(resolve));
  assert.equal(advances, 1); s.stop();
});
