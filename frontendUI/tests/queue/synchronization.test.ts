import { test } from 'node:test';
import assert from 'node:assert/strict';
import { QueueSession } from '../../app/lib/queue-state.ts';
import { fixture, deferred, queue, entry, current } from './support.ts';

test('dirty hint during a read triggers another authoritative read', async () => {
  const f = fixture(); const read = deferred<ReturnType<typeof current>>();
  f.deps.current = () => read.promise;
  const session = new QueueSession(f.deps); session.start();
  const first = session.sync(); session.sync();
  f.deps.current = async () => current(); read.resolve(current());
  await first;
  assert.ok(f.queueReads() >= 2); session.stop();
});

test('lower revisions and old mutation responses cannot regress queue', async () => {
  const f = fixture(); const session = new QueueSession(f.deps); session.start(); await session.sync();
  const epoch = session.capture();
  session.applyQueue(queue('10'), epoch);
  session.applyQueue(queue('9'), epoch);
  assert.equal(session.getSnapshot().queue?.revision, '10');
  session.stop(); session.start(); await session.sync();
  session.applyQueue(queue('999'), epoch);
  assert.notEqual(session.getSnapshot().queue?.revision, '999'); session.stop();
});

test('uncertain submission retains request ID and duplicate video occurrences remain distinct', async () => {
  const f = fixture(); const ids: string[] = [];
  f.deps.submit = async (_m, attempt) => {
    ids.push(attempt.request_id);
    if (ids.length === 1) throw new TypeError('lost response');
    return { changed: false, entry_id: 'a'.repeat(32), active: true, queue: queue('2', [entry('a'), entry('b')]) };
  };
  const session = new QueueSession(f.deps); session.start(); await session.sync();
  await session.add('https://youtu.be/dQw4w9WgXcQ');
  assert.ok(session.getSnapshot().attempt);
  await session.retrySubmission();
  assert.equal(ids[0], ids[1]); assert.equal(session.getSnapshot().attempt, null);
  assert.equal(session.getSnapshot().queue?.upcoming.length, 1); session.stop();
});

test('old terminal event cannot clear replacement membership', async () => {
  const f = fixture(); const session = new QueueSession(f.deps); session.start(); await session.sync();
  const old = session.capture();
  f.deps.current = async () => current('b'.repeat(32));
  f.deps.read = async () => queue('2', [], 'b'.repeat(32));
  await session.sync(); session.ended('closed', old);
  assert.equal(session.getSnapshot().membership?.membership_id, 'b'.repeat(32)); session.stop();
});

test('transfer discards an old in-flight queue read and discovers the new membership', async () => {
  const f = fixture();
  const session = new QueueSession(f.deps); session.start(); await session.sync();
  const old = session.capture();
  const response = deferred<ReturnType<typeof queue>>();
  f.deps.read = () => response.promise;
  const pending = session.sync();
  await new Promise(resolve => setImmediate(resolve));
  f.deps.current = async () => current('b'.repeat(32));
  f.deps.read = async () => queue('2', [entry('b')], 'b'.repeat(32));
  session.ended('left', old);
  response.resolve(queue('999', [entry('a')])); await pending;
  assert.equal(session.getSnapshot().membership?.membership_id, 'b'.repeat(32));
  assert.equal(session.getSnapshot().queue?.current?.id, entry('b').id);
  session.stop();
});

test('fallback text never submits and supported music links retain main normalization', async () => {
  const f = fixture(); const urls: string[] = [];
  f.deps.submit = async (_m, attempt) => {
    urls.push(attempt.url);
    return { changed: true, entry_id: entry().id, active: true, queue: queue('2') };
  };
  const session = new QueueSession(f.deps); session.start(); await session.sync();
  await session.add('search for a song');
  assert.equal(urls.length, 0);
  await session.add('https://music.youtube.com/watch?v=dQw4w9WgXcQ');
  assert.deepEqual(urls, ['https://www.youtube.com/watch?v=dQw4w9WgXcQ']);
  session.stop();
});

test('a different search result cannot replace an unresolved submission attempt', async () => {
  const f = fixture(); const attempts: { url: string; request_id: string }[] = [];
  f.deps.submit = async (_m, attempt) => {
    attempts.push({ ...attempt });
    if (attempts.length === 1) throw new TypeError('response lost');
    return { changed: false, entry_id: entry().id, active: true, queue: queue('2', [entry()]) };
  };
  const session = new QueueSession(f.deps); session.start(); await session.sync();
  await session.add('https://www.youtube.com/watch?v=dQw4w9WgXcQ');
  await session.add('https://www.youtube.com/watch?v=LDU_Txk06tM');
  assert.equal(attempts.length, 1);
  await session.retrySubmission();
  assert.deepEqual(attempts[1], attempts[0]);
  assert.equal(session.getSnapshot().attempt, null); session.stop();
});
