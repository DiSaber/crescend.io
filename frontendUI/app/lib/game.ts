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

export type RoundResult = {
  round: number;
  entries: { submission_id: string; song: GameSong; submitted_by_user_id: string; votes: number; winner: boolean; points: number }[];
  answer?: string | null;
  guesses: { user_id: string; rank: number; points: number }[];
};

export type NowPlaying = {
  play_seq: number;
  round: number;
  /** Name That Tune while the title is secret: song and proposer are null for guessers. */
  hidden: boolean;
  song: GameSong | null;
  submitted_by_user_id: string | null;
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
  scoreboard: { user_id: string; points: number; active: boolean }[];
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
  let code: string | undefined;
  let message = `HTTP ${response.status}`;
  try {
    const body = await response.json();
    code = body.code;
    message = body.error ?? message;
  } catch {}
  throw new QueueError(response.status, code, message);
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

export async function getGame(membershipId: string, signal?: AbortSignal): Promise<GameSnapshot> {
  return check(await apiFetch(base(membershipId), { signal }));
}
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
