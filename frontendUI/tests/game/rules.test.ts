import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  duelShortNotice, emptyRound, emptyRoundLine, limitModeName, modeNameError, modeNameLength, nudgeValue, shownPrompt, pairSummary, pickErrorMessage, playerWarning, resultGroups, winnerLine,
  type GameMode, type GameView, type ResultEntry, type RoundResult, type SettingSchema,
} from '../../app/lib/game.ts';
import { QueueError } from '../../app/lib/queue.ts';

const collect: SettingSchema = { key: 'collect_seconds', label: 'Time to pick', type: 'integer', default: 45, min: 10, max: 300, step: 5, unit: 'seconds' };

test('+/- moves one step in the pressed direction, even from an off-step value', () => {
  assert.equal(nudgeValue(collect, '47', 1), 50);
  assert.equal(nudgeValue(collect, '47', -1), 45);
  assert.equal(nudgeValue(collect, '45', 1), 50);
  assert.equal(nudgeValue(collect, '45', -1), 40);
  assert.equal(nudgeValue(collect, '43', 1), 45, 'never back the other way');
  assert.equal(nudgeValue(collect, '43', -1), 40);
  assert.equal(nudgeValue(collect, '300', 1), 300);
  assert.equal(nudgeValue(collect, '10', -1), 10);
  assert.equal(nudgeValue(collect, '999', -1), 300, 'out of range comes back inside');
  assert.equal(nudgeValue(collect, '2', 1), 10);
  assert.equal(nudgeValue(collect, '', 1), 10);
  assert.equal(nudgeValue(collect, 'abc', -1), 300);
  const odd: SettingSchema = { ...collect, min: 0, max: 7, step: 5 };
  assert.equal(nudgeValue(odd, '5', 1), 5, 'stays on the grid below an off-grid max');
});

test('mode names count characters like the server and refuse hidden ones', () => {
  const notes = '\u{1F3B5}'.repeat(41);
  const limited = limitModeName(notes);
  assert.equal(Array.from(limited.value).length, 40);
  assert.equal(limited.truncated, true);
  assert.deepEqual(limitModeName('Party night'), { value: 'Party night', truncated: false });
  assert.equal(modeNameError('\u{1F3B5}'.repeat(40)), null, 'an emoji is one character');
  assert.match(modeNameError('Duel‮nwod')!, /invisible/);
  assert.match(modeNameError('Du​el')!, /invisible/);
  assert.match(modeNameError('   ')!, /name/);
});

test('mode names follow the nickname character rules', () => {
  for (const bad of ['a\u0000b', 'a\nb', 'a\u2028b', 'a\u2029b', 'a\u00ADb', 'a\u200Cb', 'a\u2066b', 'Kat\uFE0F', 'Kat\u200D']) {
    assert.match(modeNameError(bad) ?? '', /control, invisible/, JSON.stringify(bad));
  }
  const family = '\u{1F468}\u200D\u{1F469}\u200D\u{1F467}\u200D\u{1F466}';
  const flag = '\u{1F3F3}\uFE0F\u200D\u{1F308}';
  for (const good of ['Kat\u2764\uFE0F', family, flag, family.repeat(40)]) assert.equal(modeNameError(good), null, good);
  assert.equal(modeNameLength(family.repeat(40)), 40, 'a ZWJ family is one character');
  assert.match(modeNameError(family.repeat(41)) ?? '', /up to 40/);
  // Spaces around a paste don't use up room: all 40 Bs are kept.
  const padded = `  ${'B'.repeat(40)}  `;
  assert.deepEqual(limitModeName(padded), { value: padded, truncated: false });
  assert.equal(modeNameError(padded), null);
  assert.equal(modeNameLength(padded), 40);
  const cut = limitModeName(`  ${family.repeat(41)}`);
  assert.equal(cut.truncated, true);
  assert.equal(cut.value, family.repeat(40), 'cut by whole emoji');
  assert.deepEqual(limitModeName('Party '), { value: 'Party ', truncated: false }, 'typing a space still works');
});

const entry = (id: string, group: number, votes: number, winner: boolean, outcome: ResultEntry['outcome']): ResultEntry => ({
  submission_id: id, song: { video_id: id, title: `Song ${id}`, author_name: 'A', thumbnail_url: '' },
  submitted_by_user_id: '1', submitted_by_nickname: 'Ana', votes, winner, points: outcome === 'won' ? 50 : 0, group, outcome,
});

test('overlap play shows the prompt of the song playing', () => {
  const round1 = { number: 1, text: 'Rainy', image_id: 'a'.repeat(32) };
  const round2 = { number: 2, text: 'Sunny', image_id: 'b'.repeat(32) };
  const playing = { play_seq: 1, round: 1, hidden: false, song: null, submitted_by_user_id: '1', submitted_by_nickname: 'Ana', start_seconds: 0, started_at: '', ends_at: null, prompt: round1 };
  const base = { phase: 'play', prompt: round2, now_playing: playing } as unknown as GameView;
  assert.deepEqual(shownPrompt(base), round1, 'play shows the playing round');
  assert.deepEqual(shownPrompt({ ...base, phase: 'collect' }), round2, 'collect shows the current round');
  assert.deepEqual(shownPrompt({ ...base, now_playing: null }), round2);
  assert.deepEqual(shownPrompt({ ...base, now_playing: { ...playing, prompt: null } }), round2);
});

test('an empty pick round and a short Duel get their own lines', () => {
  const duel = { status: 'running', mode: { name: 'Duel', rules: { collect: 'pick', ballot: 'pairs' }, settings: {} } } as unknown as GameView;
  const empty: RoundResult = { round: 1, entries: [], guesses: [] };
  assert.equal(emptyRound(duel, empty), true);
  assert.equal(emptyRoundLine(empty), 'Round 1: nobody picked a song.');
  assert.equal(emptyRound(duel, null), false);
  assert.equal(emptyRound({ ...duel, mode: { ...duel.mode, rules: { ...duel.mode.rules, collect: 'propose' } } }, empty), false);
  assert.equal(duelShortNotice(duel, 2), 'Duel needs 3 players for voting; remaining pairs are ties until more join.');
  assert.equal(duelShortNotice(duel, 3), null);
  assert.equal(duelShortNotice({ ...duel, status: 'ended' }, 2), null);
});

test('Duel results group by pair and say which song beat which', () => {
  const result: RoundResult = {
    round: 1, guesses: [],
    entries: [entry('a', 0, 2, true, 'won'), entry('b', 0, 1, false, 'lost'), entry('c', 1, 0, true, 'tie'), entry('d', 1, 0, false, 'tie'), entry('e', 2, 0, true, 'bye')],
  };
  const groups = resultGroups(result);
  assert.equal(groups.length, 3);
  assert.equal(pairSummary(groups[0]), '2-1: “Song a” beat “Song b”');
  assert.match(pairSummary(groups[1]), /^Tie 0-0, no points: “Song c” plays/);
  assert.match(pairSummary(groups[2]), /^Bye: “Song e”/);
});

const view = (points: number[]): GameView => ({
  scoreboard: points.map((p, i) => ({ user_id: String(i + 1), nickname: ['Ana', 'Bo', 'Cy'][i], points: p, active: true })),
}) as unknown as GameView;

test('the final line names every player sharing the top score', () => {
  const name = (_id: string, nickname: string) => nickname;
  assert.equal(winnerLine(view([100, 100, 50]), name), 'Tie: Ana and Bo share the win with 100 points.');
  assert.equal(winnerLine(view([100, 50]), name), 'Ana wins with 100 points.');
  assert.equal(winnerLine(view([0, 0]), name), 'No points this time.');
});

test('a video that cannot be described is a definite rejection with a plain message', () => {
  assert.equal(pickErrorMessage(new QueueError(502, 'metadata_unavailable', 'Video metadata unavailable.')), "That video can't be used. Try another link or search.");
  assert.equal(pickErrorMessage(new QueueError(409, 'song_already_picked', 'x')), 'Someone already picked that song this round. Pick another.');
  assert.equal(pickErrorMessage(new QueueError(502, 'upstream_unavailable', 'x')), null, 'an outage may still retry');
  assert.equal(pickErrorMessage(new Error('network')), null);
});

test('setup warns about small lobbies and blocks a 2-player Duel', () => {
  const mode = (ballot: string, challenge: string) => ({ rules: { ballot, challenge } }) as unknown as GameMode;
  assert.equal(playerWarning(mode('pairs', 'listen'), 2)?.blocks, true);
  assert.equal(playerWarning(mode('pairs', 'listen'), 3), null);
  assert.equal(playerWarning(mode('none', 'guess_title'), 2)?.blocks, false);
  assert.equal(playerWarning(mode('all', 'listen'), 2), null);
});
