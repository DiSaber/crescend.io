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

export type DiscoveryStatus =
  | "not_needed"
  | "cached"
  | "fresh"
  | "missing_credentials"
  | "daily_limit_reached"
  | "temporarily_unavailable";

export type SongSearchResponse = {
  results: SongResult[];
  discovery: { status: DiscoveryStatus };
};

/** A non-2xx search response, keeping the stable `code` (docs/api-errors.md). */
export class SongSearchError extends Error {
  constructor(
    readonly status: number,
    readonly code: string | undefined,
  ) {
    super(`Song search failed (${status}${code ? ` ${code}` : ""})`);
  }
}

/**
 * Catalog matches first, then YouTube results when the backend ran discovery. `more`
 * asks for discovery even when the catalog already has five or more matches.
 */
export async function searchSongs(q: string, more: boolean, signal?: AbortSignal): Promise<SongSearchResponse> {
  const params = new URLSearchParams({ q });
  if (more) params.set("more", "true");
  const response = await apiFetch(`/api/songs/search?${params}`, { signal });
  if (response.ok) return response.json();
  let code: string | undefined;
  try {
    code = (await response.json()).code;
  } catch {}
  throw new SongSearchError(response.status, code);
}
