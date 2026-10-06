// Lobby API client, following docs/lobby-api.md.
import { apiFetch, getToken, refresh } from "./auth";

export type Member = { user_id: string; role: "owner" | "member"; joined_at: string };
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

export async function getCurrent(): Promise<CurrentLobby> {
  const response = await apiFetch("/api/lobbies/current");
  if (!response.ok) throw new Error(await errorMessage(response));
  return response.json();
}

/** Create a lobby, or return the caller's existing one on 409. */
export async function createLobby(): Promise<CurrentLobby> {
  const response = await apiFetch("/api/lobbies", { method: "POST" });
  if (response.status === 409) return getCurrent();
  if (!response.ok) throw new Error(await errorMessage(response));
  return response.json();
}

export async function joinLobby(joinCode: string): Promise<CurrentLobby> {
  const response = await apiFetch("/api/lobbies/join", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ join_code: joinCode }),
  });
  if (response.status === 404) throw new Error("No active lobby has that code.");
  if (response.status === 409) throw new Error("You are already in another lobby. Leave it first.");
  if (!response.ok) throw new Error(await errorMessage(response));
  return response.json();
}

export async function leaveLobby(membershipId: string): Promise<void> {
  const response = await apiFetch(`/api/lobbies/memberships/${membershipId}`, { method: "DELETE" });
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
  | { event: "membership_ended"; data: { reason: "expired" | "left" | "closed" } }
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
  let token = await getToken();
  const open = () =>
    fetch(`/api/lobbies/memberships/${encodeURIComponent(membershipId)}/events`, {
      headers: { Authorization: `Bearer ${token}`, Accept: "text/event-stream" },
      cache: "no-store",
      signal,
    });
  let response = await open();
  if (response.status === 401) {
    token = await refresh();
    response = await open();
  }
  if (!response.ok || !response.body) throw new Error(`Stream HTTP ${response.status}`);

  const reader = response.body.getReader();
  const decoder = new TextDecoder("utf-8");
  let buffer = "";
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) return;
      buffer += decoder.decode(value, { stream: true });
      for (;;) {
        const boundary = /\r?\n\r?\n/.exec(buffer);
        if (!boundary) break;
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
    reader.cancel().catch(() => {});
  }
}
