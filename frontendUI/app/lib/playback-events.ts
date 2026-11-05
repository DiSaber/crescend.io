/** A failure or disposed player's callbacks cannot consume a queue occurrence. */
export function playbackEvents(callbacks: { ended: () => void; error: (message: string) => void; playing: () => void }) {
  let disposed = false, ended = false, failed = false;
  return {
    state(event: { data: number }) {
      if (disposed) return;
      if (event.data === 1) { failed = false; callbacks.playing(); }
      if (event.data === 0 && !ended && !failed) { ended = true; callbacks.ended(); }
    },
    error() {
      if (disposed) return;
      failed = true; callbacks.error("YouTube could not play this video here. The song is still queued.");
    },
    blocked() {
      if (disposed) return;
      failed = true; callbacks.error("Playback needs your permission. Press Play to continue.");
    },
    dispose() { disposed = true; },
  };
}
