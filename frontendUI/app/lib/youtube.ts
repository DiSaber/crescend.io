// Client-side mirror of the backend's supported-link check (backend/src/youtube/mod.rs),
// so text that can't be a video link never reaches the queue API.

const VIDEO_ID = /^[A-Za-z0-9_-]{6,24}$/;

/**
 * The link to submit for a supported YouTube video URL, or null when `input` isn't one.
 * Accepts youtu.be, watch, shorts and embed links on youtube.com / www / m, plus
 * music.youtube.com watch links, which are rewritten to www.youtube.com because the
 * backend doesn't accept that host.
 */
export function youTubeVideoUrl(input: string): string | null {
  let url: URL;
  try {
    url = new URL(input.trim());
  } catch {
    return null;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;

  const host = url.hostname.toLowerCase();
  let id: string | null = null;
  if (host === "youtu.be") {
    id = url.pathname.split("/")[1] ?? null;
  } else if (host === "music.youtube.com") {
    if (url.pathname.replace(/^\/+|\/+$/g, "") === "watch") id = url.searchParams.get("v");
    if (id !== null && VIDEO_ID.test(id)) return `https://www.youtube.com/watch?v=${id}`;
    return null;
  } else if (host === "youtube.com" || host === "www.youtube.com" || host === "m.youtube.com") {
    const path = url.pathname.replace(/^\/+|\/+$/g, "");
    if (path === "watch") id = url.searchParams.get("v");
    else if (path.startsWith("shorts/")) id = path.slice("shorts/".length);
    else if (path.startsWith("embed/")) id = path.slice("embed/".length);
  }
  return id !== null && VIDEO_ID.test(id) ? input.trim() : null;
}

export function youTubeSearchUrl(query: string): string {
  return `https://www.youtube.com/results?search_query=${encodeURIComponent(query.trim())}`;
}
