// Song search client for GET /api/songs/search, following docs/song-search-api.md.
import { apiFetch } from "./auth";

export type SongResult = {
  video_id: string;
  /** Plain text; render as text. */
  title: string;
  artist: string;
  thumbnail_url: string | null;
  /** Submit through the queue API exactly like a pasted link. */
  watch_url: string;
  source: "catalog" | "youtube" | "youtube_cache";
};

/** Saved and starter songs first, then any external results the backend already has. */
export async function searchSongs(q: string, signal?: AbortSignal): Promise<SongResult[]> {
  const response = await apiFetch(`/api/songs/search?${new URLSearchParams({ q })}`, { signal });
  if (!response.ok) throw new Error(`Song search failed (${response.status})`);
  return (await response.json()).results;
}
