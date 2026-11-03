import { test } from 'node:test';
import assert from 'node:assert/strict';
import { playbackEvents } from '../../app/lib/playback-events.ts';

test('player error and autoplay rejection preserve occurrence even if an ended event follows', () => {
  let ends = 0; const errors: string[] = [];
  const events = playbackEvents({ ended: () => ends++, error: message => errors.push(message), playing: () => {} });
  events.error(); events.state({ data: 0 }); events.blocked(); events.state({ data: 0 });
  assert.equal(ends, 0); assert.equal(errors.length, 2);
  events.state({ data: 1 }); events.state({ data: 0 }); events.state({ data: 0 });
  assert.equal(ends, 1);
});
test('disposed player callbacks cannot advance the next occurrence of the same video', () => {
  const ended: string[] = [];
  const first = playbackEvents({ ended: () => ended.push('occurrence-a'), error: () => {}, playing: () => {} });
  first.dispose();
  const second = playbackEvents({ ended: () => ended.push('occurrence-b'), error: () => {}, playing: () => {} });
  first.state({ data: 0 }); second.state({ data: 0 });
  assert.deepEqual(ended, ['occurrence-b']);
});
