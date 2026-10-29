"use client";

import { useEffect, useState } from "react";
import YouTubeSearchFallback from "./YouTubeSearchFallback";
import { searchSongs, type DiscoveryStatus, type SongResult } from "../lib/songs";
import { youTubeSearchUrl } from "../lib/youtube";

const DEBOUNCE_MS = 250;
const MIN_CHARS = 2;

// Why YouTube results may be missing (docs/song-search-api.md).
const DISCOVERY_SKIPPED: Partial<Record<DiscoveryStatus, string>> = {
  missing_credentials: "YouTube search isn't set up here",
  daily_limit_reached: "YouTube search is used up for today",
  temporarily_unavailable: "YouTube search didn't respond",
};

type Found = { key: string; q: string; results: SongResult[]; status: DiscoveryStatus } | { key: string; q: string; error: true };

const keyOf = (q: string, more: boolean) => `${more ? "more" : "first"}:${q}`;

/**
 * Search-as-you-type for text that isn't a video link, on GET /api/songs/search (#51).
 * Saved songs come first, then YouTube results when the backend ran discovery; "Search
 * YouTube for more" asks for discovery when the catalog alone had enough matches. Add hands
 * the result's watch URL to the lobby's normal submit flow, exactly like a pasted link.
 * No results, or search unavailable, falls back to searching YouTube by hand (#47).
 */
export default function SongSearch({
  query,
  addingUrl,
  onAdd,
  onPasteLink,
}: {
  query: string;
  /** The URL being submitted right now, if any; disables every Add button meanwhile. */
  addingUrl?: string;
  onAdd: (url: string) => void;
  onPasteLink?: (text: string) => void;
}) {
  const q = query.trim();
  // The query the user asked "more" for; typing something else drops it.
  const [moreFor, setMoreFor] = useState<string>();
  const more = moreFor === q;
  const key = keyOf(q, more);
  // Responses are tagged with the request they answer, so a slow reply never shows
  // under newer text.
  const [found, setFound] = useState<Found>({ key: "", q: "", results: [], status: "not_needed" });

  useEffect(() => {
    if (q.length < MIN_CHARS) return;
    const controller = new AbortController();
    const requestKey = keyOf(q, more);
    const timer = setTimeout(
      () => {
        searchSongs(q, more, controller.signal)
          .then((response) => setFound({ key: requestKey, q, results: response.results, status: response.discovery.status }))
          .catch(() => {
            if (!controller.signal.aborted) setFound({ key: requestKey, q, error: true });
          });
      },
      more ? 0 : DEBOUNCE_MS,
    );
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [q, more]);

  if (q.length < MIN_CHARS) return null;
  const settled = found.key === key;
  // While a new search runs, keep showing (dimmed) only results for text this one extends
  // or trims, never results for an unrelated earlier search.
  const related = q.startsWith(found.q) || found.q.startsWith(q);
  const results = "error" in found || (!settled && !related) ? null : found.results;

  if (settled && !results?.length) {
    const skipped = "error" in found ? undefined : DISCOVERY_SKIPPED[found.status];
    const note =
      "error" in found ? "Song search is unavailable right now." : `No songs match “${q}”.${skipped ? ` ${skipped}.` : ""}`;
    return (
      <div className="w-full flex flex-col gap-2">
        <p className="text-sm text-zinc-400">{note}</p>
        <YouTubeSearchFallback query={q} onPasteLink={onPasteLink} />
      </div>
    );
  }
  if (!results?.length) {
    return <p className="text-sm text-zinc-500" aria-live="polite">Searching…</p>;
  }

  const status = "error" in found ? undefined : found.status;
  return (
    <div className="w-full flex flex-col gap-2">
      <ul
        className={`flex flex-col gap-2 transition-opacity ${settled ? "" : "opacity-60"}`}
        aria-busy={!settled}
        aria-label="Song search results"
      >
        {results.map((song) => (
          <li key={`${song.source}-${song.video_id}`} className="flex items-center gap-3 rounded-lg bg-zinc-800 p-2">
            {song.thumbnail_url ? (
              <img src={song.thumbnail_url} alt="" className="h-12 w-20 shrink-0 rounded object-cover" />
            ) : (
              <div className="h-12 w-20 shrink-0 rounded bg-zinc-700" aria-hidden="true" />
            )}
            <div className="min-w-0 flex-1">
              <p className="truncate text-sm font-medium">{song.title}</p>
              <p className="truncate text-xs text-zinc-400">
                {song.source !== "catalog" && <span className="mr-1 rounded bg-zinc-700 px-1 text-[10px] uppercase">YouTube</span>}
                {song.artist}
              </p>
            </div>
            <button
              type="button"
              onClick={() => onAdd(song.watch_url)}
              disabled={!!addingUrl}
              aria-label={`Add ${song.title}`}
              className="min-h-11 min-w-11 shrink-0 px-4 rounded-lg bg-[#5C172E] hover:bg-[#FF0048] disabled:opacity-50 transition font-semibold"
            >
              {addingUrl === song.watch_url ? "Adding…" : "Add"}
            </button>
          </li>
        ))}
      </ul>
      {settled && status === "not_needed" && (
        <button
          type="button"
          onClick={() => setMoreFor(q)}
          className="min-h-11 px-4 rounded-lg bg-zinc-800 border border-zinc-700 hover:border-[#FF0048] transition font-semibold"
        >
          Search YouTube for more
        </button>
      )}
      {more && !settled && <p className="text-sm text-zinc-500" aria-live="polite">Searching YouTube…</p>}
      {settled && status && DISCOVERY_SKIPPED[status] && (
        <p className="text-sm text-zinc-500">{DISCOVERY_SKIPPED[status]}, so these are saved songs only.</p>
      )}
      <p className="text-sm text-zinc-400">
        Not listed?{" "}
        <a href={youTubeSearchUrl(q)} target="_blank" rel="noopener noreferrer" className="py-3.5 underline hover:text-white">
          Search YouTube
        </a>{" "}
        and paste the video&apos;s link above.
      </p>
    </div>
  );
}
