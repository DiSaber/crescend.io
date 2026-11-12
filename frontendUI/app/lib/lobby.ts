// Lobby API client, following docs/lobby-api.md.
import { apiFetch } from "./auth.ts";

/** `nickname` is plain text chosen by the member: render it as text, never as HTML. */
export type Member = {
  user_id: string; role: "owner" | "member"; nickname: string; joined_at: string;
  /** No open stream for over a minute: the game and skip votes stop waiting for them. */
  away?: boolean;
};
export type Lobby = {
  id: string;
  join_code: string;
  owner_user_id: string;
  created_at: string;
  expires_at: string;
  revision: string;
  members: Member[];
};
export type CurrentLobby = { membership_id: string; lobby: Lobby } | { membership_id: null; lobby: null };

export type VideoMetadata = {
  video_id: string;
  title: string;
  author_name: string;
  author_url: string;
  thumbnail_url: string;
  embed_url: string;
};

async function errorMessage(response: Response): Promise<string> {
  try {
    const body = await response.json();
    return body.message ?? body.error ?? `HTTP ${response.status}`;
  } catch {
    return `HTTP ${response.status}`;
  }
}

export async function getCurrent(signal?: AbortSignal): Promise<CurrentLobby> {
  const response = await apiFetch("/api/lobbies/current", { signal });
  if (!response.ok) throw new Error(await errorMessage(response));
  return response.json();
}

/** Same rules as the server (backend nicknames::normalize): 1-24 user-perceived characters. */
export const NICKNAME_MAX = 24;
const MAX_SCALARS_PER_CHAR = 12;
/** Whitespace runs collapse to one space, as the server stores the name. */
export const normalizeNickname = (raw: string) => raw.trim().split(/\s+/).join(" ");
// Emoji pictographs (approximates Extended_Pictographic, like the server).
const PICTOGRAPHIC = /[\u00a9\u00ae\u203c\u2049\u2122\u2139\u2194-\u21ff\u2300-\u23ff\u24c2\u25a0-\u27bf\u2900-\u297f\u2b00-\u2bff\u3030\u303d\u3297\u3299\u{1F000}-\u{1FAFF}]/u;
const FORBIDDEN = /[\u0000-\u001f\u007f-\u009f\u00ad\u061c\u180e\u200b-\u200f\u2028-\u202e\u2060-\u206f\ufeff\ufff9-\ufffb\ufe00-\ufe0f]/u;
/** User-perceived characters (grapheme clusters): a family emoji counts once. */
export function nicknameLength(name: string): number {
  return graphemes(name).length;
}
/** Grapheme clusters, the characters a person sees. */
export function graphemes(name: string): string[] {
  if (typeof Intl !== "undefined" && "Segmenter" in Intl) {
    return Array.from(new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(name), s => s.segment);
  }
  return Array.from(name);
}
/**
 * Whether a display name (nickname or mode name) has a control, invisible or
 * direction-changing character, or a joiner or variation selector outside an emoji.
 */
export function hiddenCharacter(raw: string): boolean {
  const chars = Array.from(raw);
  return chars.some((c, i) => {
    const before = chars[i - 1];
    const after = chars[i + 1];
    // A zero-width joiner only inside an emoji sequence (👨‍👩‍👧, 🏳️‍🌈).
    if (c === "\u200d") return !(before && (PICTOGRAPHIC.test(before) || before === "\ufe0f" || before === "\u{E007F}") && after && PICTOGRAPHIC.test(after));
    // A presentation selector only right after an emoji or a keycap base.
    if (c === "\ufe0e" || c === "\ufe0f") return !(before && (PICTOGRAPHIC.test(before) || /[0-9#*]/.test(before)));
    return FORBIDDEN.test(c);
  });
}
/** One character stacking more code points than the server allows. */
export const stacked = (chars: string[]) => chars.some(c => Array.from(c).length > MAX_SCALARS_PER_CHAR);

export function nicknameProblem(raw: string): string | undefined {
  const name = normalizeNickname(raw);
  if (!name) return "Pick a nickname.";
  if (hiddenCharacter(raw)) return "Nicknames can't contain invisible or control characters.";
  const chars = graphemes(name);
  if (chars.length > NICKNAME_MAX) return `Nicknames are at most ${NICKNAME_MAX} characters.`;
  if (stacked(chars)) return "That nickname has too many stacked marks.";
  return undefined;
}

/**
 * The six-character code inside whatever was typed or pasted: spaces, dashes and
 * other separators drop out, letters are upper-cased, and a pasted join link
 * (".../join/?code=ABC123") gives its code.
 */
export function normalizeCode(raw: string): string {
  const linked = /[?&]code=([^&#\s]*)/i.exec(raw);
  return (linked ? decodeURIComponent(linked[1]) : raw).toUpperCase().replace(/[^A-Z0-9]/g, "").slice(0, 6);
}

/** Create a lobby, or return the caller's existing one on 409. */
export async function createLobby(nickname?: string): Promise<CurrentLobby> {
  const response = await apiFetch("/api/lobbies", nickname === undefined ? { method: "POST" } : {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ nickname }),
  });
  if (response.status === 403) throw new Error("Hosting needs a Google account. Guests can join with a code.");
  if (response.status === 409) return getCurrent();
  if (!response.ok) throw new Error(await errorMessage(response));
  return response.json();
}

export async function joinLobby(joinCode: string, nickname?: string): Promise<CurrentLobby> {
  const body = JSON.stringify(nickname === undefined ? { join_code: joinCode } : { join_code: joinCode, nickname });
  const response = await apiFetch("/api/lobbies/join", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body,
  });
  if (response.status === 404) throw new Error("No active session has that code. Check it and try again.");
  if (response.status === 409) {
    const current = await getCurrent();
    if (!current.membership_id) throw new Error("Your session changed. Try joining again.");
    if (!window.confirm(`You're hosting another session (${current.lobby.join_code}). End that session for everyone and join this one?`)) {
      throw new Error("Your current session is still active.");
    }
    await leaveLobby(current.membership_id);
    const retry = await apiFetch("/api/lobbies/join", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body,
    });
    if (!retry.ok) throw new Error(await errorMessage(retry));
    return retry.json();
  }
  if (!response.ok) throw new Error(await errorMessage(response));
  return response.json();
}

export async function leaveLobby(membershipId: string): Promise<void> {
  const response = await apiFetch(`/api/lobbies/memberships/${membershipId}`, { method: "DELETE" });
  if (response.status !== 204) throw new Error(await errorMessage(response));
}

/** sessionStorage key the join page sets and the lobby page reads once:
 * "You're 'Kat 2' because 'Kat' is taken." */
export const JOIN_NOTICE_KEY = "crescend-join-notice";

/** Host only: take a member out of the session, like their leaving. They can rejoin. */
export async function removeMember(membershipId: string, userId: string): Promise<void> {
  const response = await apiFetch(`/api/lobbies/memberships/${membershipId}/members/${userId}`, { method: "DELETE" });
  if (response.status !== 204) throw new Error(await errorMessage(response));
}

export async function fetchMetadata(url: string): Promise<VideoMetadata> {
  const response = await fetch("/api/youtube/metadata", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ url }),
  });
  if (response.status === 422) throw new Error("That isn't a YouTube video link.");
  if (!response.ok) throw new Error(await errorMessage(response));
  return response.json();
}

export type LobbyEvent =
  | { event: "sync_required" | "lobby_changed"; data: { revision: string } }
  | { event: "membership_ended"; data: { reason: "expired" | "left" | "closed" | "removed" } }
  | { event: "auth_expired"; data: Record<string, never> };

/**
 * Read the membership event stream until it ends or `signal` aborts.
 * Resolves on a clean end; rejects on HTTP or network failure so the caller can reconnect.
 */
export async function streamLobbyEvents(
  membershipId: string,
  signal: AbortSignal,
  onEvent: (e: LobbyEvent) => void,
): Promise<void> {
  const response = await apiFetch(`/api/lobbies/memberships/${encodeURIComponent(membershipId)}/events`, {
    headers: { Accept: "text/event-stream" }, signal,
  });
  if (!response.ok || !response.body) throw new Error(`Stream HTTP ${response.status}`);

  const reader = response.body.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let buffer = "";
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) return;
      buffer += decoder.decode(value, { stream: true });
      for (;;) {
        const boundary = /\r?\n\r?\n/.exec(buffer);
        if (!boundary) {
          if (buffer.length > 65536) throw new Error("Oversized lobby event");
          break;
        }
        if (boundary.index > 65536) throw new Error("Oversized lobby event");
        const block = buffer.slice(0, boundary.index);
        buffer = buffer.slice(boundary.index + boundary[0].length);
        let event = "message";
        const data: string[] = [];
        for (const line of block.split(/\r?\n/)) {
          if (line.startsWith("event:")) event = line.slice(6).replace(/^ /, "");
          if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""));
        }
        if (data.length) onEvent({ event, data: JSON.parse(data.join("\n")) } as LobbyEvent);
      }
    }
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
