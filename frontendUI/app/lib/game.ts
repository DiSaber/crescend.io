// Game API client, following docs/game-api.md.
import { apiFetch } from "./auth.ts";
import { QueueError } from "./queue.ts";

export type Rules = {
  prompt: "none" | "text" | "image";
  collect: "pick" | "propose";
  ballot: "all" | "pairs" | "none";
  winner: "most_votes" | "pair_winners" | "none";
  challenge: "listen" | "guess_title";
  scoring: "votes_to_picker" | "winner_bonus" | "guess_speed";
};

export type SettingSchema = {
  key: string;
  label: string;
  type: "integer" | "boolean";
  default: number | boolean;
  min: number | null;
  max: number | null;
  step: number | null;
  unit: string | null;
};

export type Prompt = { text?: string | null; image_id?: string | null };

export type GameMode = {
  /** Preset key such as `party`, or a saved mode's 32-hex ID. */
  id: string;
  name: string;
  description: string;
  preset: boolean;
  rules: Rules;
  settings: SettingSchema[];
  default_prompts: Prompt[];
};

export type SettingValues = Record<string, number | boolean>;

export type GameSong = {
  video_id: string; title: string; author_name: string; thumbnail_url: string; embed_url?: string;
};

export type Phase = "collect" | "vote" | "play" | "guess" | "reveal" | "ended";

export type BallotEntry = { submission_id: string; song: GameSong; mine: boolean; votable: boolean };

/** `tie`: the pair ended level (0-0 too), its earliest song plays, nobody scores. */
export type Outcome = "won" | "lost" | "tie" | "bye";
export type ResultEntry = {
  submission_id: string; song: GameSong; submitted_by_user_id: string; submitted_by_nickname: string;
  votes: number;
  /** The song plays (a won pair, a tie's earliest song, or a bye). */
  winner: boolean;
  /** Points this song earned its submitter. */
  points: number;
  /** Pair ballots: the 0-based pair. */
  group: number;
  outcome: Outcome;
};
export type RoundResult = {
  round: number;
  entries: ResultEntry[];
  answer?: string | null;
  guesses: { user_id: string; nickname: string; rank: number; points: number }[];
  /** Name That Tune: why there was no guessing. */
  note?: "nobody_to_guess" | "no_proposals" | "ended_early" | null;
};

export type NowPlaying = {
  play_seq: number;
  round: number;
  /** Name That Tune while the title is secret: song and proposer are null for guessers. */
  hidden: boolean;
  song: GameSong | null;
  submitted_by_user_id: string | null;
  submitted_by_nickname: string | null;
  start_seconds: number;
  started_at: string;
  /** Null plays the full song and the host reports its end. */
  ends_at: string | null;
};

export type GuessStanding = {
  eligible: boolean; own_song: boolean; correct: boolean; points: number;
  wrong_guesses: number; guesses_left: number; correct_count: number;
};

export type GameView = {
  id: string;
  mode_id: string;
  mode: { name: string; rules: Rules; settings: SettingValues };
  status: "running" | "ended";
  round: number;
  total_rounds: number | null;
  phase: Phase;
  phase_seq: number;
  phase_ends_at: string | null;
  prompt: { number: number; text: string | null; image_id: string | null } | null;
  my_submissions: { id: string; song: GameSong }[];
  submission_count: number;
  ballot: { groups: { entries: BallotEntry[] }[]; votes_allowed: number; my_votes: string[] } | null;
  last_result: RoundResult | null;
  now_playing: NowPlaying | null;
  guess: GuessStanding | null;
  up_next_count: number;
  scoreboard: { user_id: string; nickname: string; points: number; active: boolean }[];
};

export type GameSnapshot = {
  membership_id: string;
  lobby_id: string;
  /** The lobby revision, shared with the queue. Decimal string; compare with BigInt. */
  revision: string;
  server_time: string;
  game: GameView | null;
};

export type GameMutation = { changed: boolean; game: GameSnapshot };
export type StartRequest = ({ mode_id: string } | { custom: { name: string; rules: Rules } }) & {
  settings?: SettingValues; prompts?: Prompt[];
};
export type UploadedImage = { image_id: string; content_type: string; width: number; height: number; bytes: number };

async function check<T>(response: Response): Promise<T> {
  if (response.ok) return response.status === 204 ? (undefined as T) : response.json();
  let code: string | undefined, setting: string | undefined, detail: string | undefined;
  let message = `HTTP ${response.status}`;
  try {
    const body = await response.json();
    code = body.code;
    message = body.error ?? message;
    if (typeof body.setting === "string") setting = body.setting;
    if (typeof body.detail === "string") detail = body.detail;
  } catch {}
  throw new QueueError(response.status, code, message, setting, detail);
}

const base = (membershipId: string) => `/api/lobbies/memberships/${encodeURIComponent(membershipId)}/game`;
const json = (body: unknown, method = "POST"): RequestInit => ({
  method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(body),
});

export async function listModes(signal?: AbortSignal): Promise<GameMode[]> {
  return (await check<{ modes: GameMode[] }>(await apiFetch("/api/game-modes", { signal }))).modes;
}
export async function saveMode(body: { name: string; rules: Rules; settings: SettingValues; prompts?: Prompt[] }) {
  return check<GameMode>(await apiFetch("/api/game-modes", json(body)));
}
export async function deleteMode(modeId: string) {
  await check<void>(await apiFetch(`/api/game-modes/${encodeURIComponent(modeId)}`, { method: "DELETE" }));
}

/** `board`: the TV view, which gets what guests see even on the host's account. */
export async function getGame(membershipId: string, signal?: AbortSignal, view?: "board"): Promise<GameSnapshot> {
  return check(await apiFetch(`${base(membershipId)}${view ? `?view=${view}` : ""}`, { signal }));
}
export const getBoardGame = (membershipId: string, signal?: AbortSignal) => getGame(membershipId, signal, "board");
export async function startGame(membershipId: string, body: StartRequest) {
  return check<GameMutation>(await apiFetch(base(membershipId), json(body)));
}
/** Ends a running game, or dismisses an ended one. */
export async function endGame(membershipId: string) {
  return check<GameMutation>(await apiFetch(base(membershipId), { method: "DELETE" }));
}
export async function advanceGame(membershipId: string, body: { phase_seq: number } | { play_seq: number }) {
  return check<GameMutation>(await apiFetch(`${base(membershipId)}/advance`, json(body)));
}
export async function submitSong(membershipId: string, url: string, requestId: string) {
  return check<GameMutation & { submission_id: string }>(
    await apiFetch(`${base(membershipId)}/submissions`, json({ url, request_id: requestId })),
  );
}
export async function castVote(membershipId: string, submissionId: string) {
  return check<GameMutation>(await apiFetch(`${base(membershipId)}/votes`, json({ submission_id: submissionId })));
}
export async function guessTitle(membershipId: string, text: string) {
  return check<GameMutation & { correct: boolean; points: number }>(
    await apiFetch(`${base(membershipId)}/guesses`, json({ text })),
  );
}

export const IMAGE_TYPES = ["image/jpeg", "image/png", "image/webp"];
export const IMAGE_MAX_BYTES = 5 * 1024 * 1024;

export async function uploadImage(membershipId: string, file: Blob) {
  const form = new FormData();
  form.append("image", file);
  return check<UploadedImage>(await apiFetch(`${base(membershipId)}/images`, { method: "POST", body: form }));
}
export async function deleteImage(membershipId: string, imageId: string) {
  await check<void>(await apiFetch(`${base(membershipId)}/images/${encodeURIComponent(imageId)}`, { method: "DELETE" }));
}
/** The route needs the bearer token, so images are fetched and shown as blob URLs. */
export async function fetchImage(membershipId: string, imageId: string, signal?: AbortSignal): Promise<Blob> {
  const response = await apiFetch(`${base(membershipId)}/images/${encodeURIComponent(imageId)}`, { signal });
  if (!response.ok) await check(response);
  return response.blob();
}

/** A field-level message for a server `invalid_game_settings` rejection, keyed by setting. */
export function serverSettingErrors(error: unknown): Record<string, string> {
  if (!(error instanceof QueueError) || error.code !== "invalid_game_settings" || !error.setting) return {};
  const detail = error.detail?.trim() || "is not allowed for this mode";
  return { [error.setting]: `${detail[0].toUpperCase()}${detail.slice(1)}${/[.!?]$/.test(detail) ? "" : "."}` };
}

/** Starting values for a customize form: each schema entry's default. */
export function defaultValues(schema: SettingSchema[]): SettingValues {
  return Object.fromEntries(schema.map(s => [s.key, s.default]));
}

/** The same check the server applies (docs/game-api.md#modes), or null when valid. */
export function settingError(spec: SettingSchema, value: unknown): string | null {
  if (spec.type === "boolean") return typeof value === "boolean" ? null : "Choose on or off.";
  if (typeof value !== "number" || !Number.isInteger(value)) return "Enter a whole number.";
  const min = spec.min ?? 0, max = spec.max ?? Number.MAX_SAFE_INTEGER, step = spec.step ?? 1;
  if (value < min || value > max) return `Use ${min} to ${max}.`;
  if ((value - min) % step !== 0) return `Use steps of ${step} (${min}, ${min + step}, ${min + 2 * step}…).`;
  return null;
}

/**
 * The step a +/- press lands on: the next value on the step grid in the pressed
 * direction, so an off-grid 47 (steps of 5 from 10) goes to 50 or 45, never back
 * the other way. Out-of-range or unparsable values come back inside the range.
 */
export function nudgeValue(spec: SettingSchema, raw: string, dir: 1 | -1): number {
  const step = spec.step ?? 1, min = spec.min ?? 0, max = spec.max ?? Number.MAX_SAFE_INTEGER;
  const n = /^\s*-?\d+\s*$/.test(raw) ? Number(raw) : NaN;
  if (!Number.isFinite(n)) return dir > 0 ? min : max;
  const index = (n - min) / step;
  const next = min + (dir > 0 ? Math.floor(index) + 1 : Math.ceil(index) - 1) * step;
  // The top of the range may sit off the grid; stop at the last grid value below it.
  const top = min + Math.floor((max - min) / step) * step;
  return Math.min(top, Math.max(min, next));
}

/** The players sharing the top score, when anyone scored. */
export function leaders(game: GameView) {
  const best = game.scoreboard[0]?.points ?? 0;
  return best > 0 ? game.scoreboard.filter(s => s.points === best) : [];
}

/** "Ana", "Ana and Bo", "Ana, Bo and Cy". */
export function joinNames(names: string[]) {
  return names.length <= 1 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** The game's end line: one winner, a shared win, or nobody scored. */
export function winnerLine(game: GameView, name: (id: string, nickname: string) => string) {
  const top = leaders(game);
  if (top.length === 0) return "No points this time.";
  const names = joinNames(top.map(t => name(t.user_id, t.nickname)));
  return top.length > 1 ? `Tie: ${names} share the win with ${top[0].points} points.` : `${names} wins with ${top[0].points} points.`;
}

/** Result entries grouped by pair (Duel) in pair order; one ballot is one group. */
export function resultGroups(result: RoundResult) {
  const groups = new Map<number, ResultEntry[]>();
  for (const e of result.entries) groups.set(e.group ?? 0, [...(groups.get(e.group ?? 0) ?? []), e]);
  return [...groups.entries()].sort((a, b) => a[0] - b[0]).map(([, entries]) => entries);
}

/** Longest mode name, counted like the server (Unicode scalar values after trimming). */
export const MODE_NAME_MAX = 40;
/** Bidirectional marks, overrides and isolates, zero-width characters and the BOM. */
const HIDDEN = /[\u200B-\u200F\u202A-\u202E\u2060-\u2064\u2066-\u2069\uFEFF\u061C]/;

/** Caps typed or pasted text at 40 characters (an emoji is one), saying when it cut. */
export function limitModeName(text: string): { value: string; truncated: boolean } {
  const chars = Array.from(text);
  return chars.length > MODE_NAME_MAX ? { value: chars.slice(0, MODE_NAME_MAX).join(""), truncated: true } : { value: text, truncated: false };
}

/** Why the server would refuse this name, or null. */
export function modeNameError(text: string): string | null {
  if (HIDDEN.test(text)) return "Mode names can't contain invisible or direction-changing characters.";
  const length = Array.from(text.trim()).length;
  if (length === 0) return "Give the mode a name.";
  if (length > MODE_NAME_MAX) return `Mode names are up to ${MODE_NAME_MAX} characters.`;
  return null;
}

/** Players a mode needs: Duel cannot vote with two, and Name That Tune is thin. */
export function playerWarning(mode: GameMode, players: number): { text: string; blocks: boolean } | null {
  if (mode.rules.ballot === "pairs" && players < 3) {
    return { text: `Duel needs at least 3 players; there ${players === 1 ? "is 1" : `are ${players}`} here. With 2, nobody can vote in the pair.`, blocks: true };
  }
  if (mode.rules.challenge === "guess_title" && players < 3) {
    return { text: "Name That Tune works best with 3 or more players. The host never guesses, so with 2 the other player's own songs are skipped.", blocks: false };
  }
  return null;
}

/** Plain-language messages for a refused pick or proposal. */
export function pickErrorMessage(error: unknown): string | null {
  if (!(error instanceof QueueError)) return null;
  if (error.code === "metadata_unavailable" || error.code === "invalid_url") return "That video can't be used. Try another link or search.";
  if (error.code === "song_already_picked") return "Someone already picked that song this round. Pick another.";
  if (error.code === "song_already_proposed") return "That song is already in this game. Pick another.";
  return null;
}

/** "“A” beat “B” 2-1", "Tie 1-1, no points: “A” plays", "Bye: “A” plays unopposed". */
export function pairSummary(group: ResultEntry[]) {
  const [a, b] = group;
  if (!b || a.outcome === "bye") return `Bye: “${a.song.title}” plays unopposed`;
  const winner = group.find(e => e.winner) ?? a;
  const other = group.find(e => e !== winner) ?? b;
  if (winner.outcome === "tie") return `Tie ${winner.votes}-${other.votes}, no points: “${winner.song.title}” plays (picked first)`;
  return `“${winner.song.title}” beat “${other.song.title}” ${winner.votes}-${other.votes}`;
}
