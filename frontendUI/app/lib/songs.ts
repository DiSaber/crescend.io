// Song catalog search client, following docs/song-catalog-api.md.
import { apiFetch } from "./auth";

export type SongResult = {
  video_id: string;
  title: string;
  artist: string;
  thumbnail_url: string;
  /** Watch URL to submit through the queue API, like a pasted link. */
  url: string;
  times_queued: number;
};

export type SongSearchResponse = {
  results: SongResult[];
  /** Only "not_configured" until the YouTube Data API fallback (#41 part 2) lands. */
  youtube: string;
};

export async function searchSongs(q: string, signal?: AbortSignal): Promise<SongSearchResponse> {
  const params = new URLSearchParams({ q, limit: "8" });
  const response = await apiFetch(`/api/songs/search?${params}`, { signal });
  if (!response.ok) throw new Error(`Search failed: ${response.status}`);
  return response.json();
}
