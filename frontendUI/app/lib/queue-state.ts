import { youTubeVideoUrl } from "./youtube.ts";
import { currentUserId, refresh } from "./auth.ts";
import { getCurrent, leaveLobby, streamLobbyEvents, type CurrentLobby, type LobbyEvent } from "./lobby.ts";
import {
  getQueue, submitEntry, advance, veto, vote, moveEntry, setVotingPolicy, newRequestId,
  QueueError, type AdvanceReason, type QueueView, type Mutation,
} from "./queue.ts";
import { getGame, type GameMutation, type GameSnapshot } from "./game.ts";

export type Attempt = { url: string; request_id: string };
export type Epoch = { version: number; membership: string | null };
export type Policy = { mode: "majority" } | { mode: "fixed"; count: number };
export type SessionDependencies = {
  current: (signal: AbortSignal) => Promise<CurrentLobby>;
  read: (membership: string, signal: AbortSignal) => Promise<QueueView>;
  identity: () => Promise<string | undefined>;
  events: typeof streamLobbyEvents;
  refresh: typeof refresh;
  submit: (membership: string, attempt: Attempt) => Promise<Mutation & { entry_id: string; active: boolean }>;
  advance: typeof advance;
  veto: typeof veto;
  vote: typeof vote;
  move: typeof moveEntry;
  policy: typeof setVotingPolicy;
  leave: typeof leaveLobby;
  /** Omitted: the session never reads the game (queue-only tests). */
  readGame?: (membership: string, signal: AbortSignal) => Promise<GameSnapshot>;
};
const defaults: SessionDependencies = {
  current: getCurrent, read: getQueue, identity: currentUserId, events: streamLobbyEvents,
  refresh, submit: (m, a) => submitEntry(m, a.url, a.request_id), advance, veto, vote,
  move: moveEntry, policy: setVotingPolicy, leave: leaveLobby, readGame: getGame,
};
export type SessionSnapshot = {
  membership: CurrentLobby | undefined;
  queue: QueueView | null;
  userId: string | undefined;
  live: boolean;
  loading: boolean;
  busy: string | null;
  error: string | null;
  syncError: string | null;
  ended: string | null;
  attempt: Attempt | null;
  game: GameSnapshot | null;
  /** Server clock minus device clock in ms, from the latest game snapshot. */
  clockOffset: number;
  gameBusy: string | null;
  gameError: string | null;
};
const endedMessages = { left: "You left the session.", closed: "The host closed the session.", expired: "The session expired." };
const message = (e: unknown) => e instanceof Error ? e.message : "Request failed. Please try again.";

/** Owns one mounted lobby view. All asynchronous completions carry a local epoch. */
export class QueueSession {
  private deps: SessionDependencies;
  private state: SessionSnapshot = {
    membership: undefined, queue: null, userId: undefined, live: false,
    loading: true, busy: null, error: null, syncError: null, ended: null, attempt: null,
    game: null, clockOffset: 0, gameBusy: null, gameError: null,
  };
  private listeners = new Set<() => void>();
  private active = false;
  private version = 0;
  private lifetime = new AbortController();
  private connection: AbortController | undefined;
  private dirty = false;
  private flight: Promise<void> | undefined;
  private pendingEnd: { entryId: string; epoch: Epoch } | undefined;
  /** The membership this client is leaving; its own end event must not read as "closed". */
  private leaving: string | null = null;
  constructor(deps: SessionDependencies = defaults) { this.deps = deps; }
  getSnapshot = () => this.state;
  subscribe = (fn: () => void) => { this.listeners.add(fn); return () => { this.listeners.delete(fn); }; };
  private update(patch: Partial<SessionSnapshot>) {
    this.state = { ...this.state, ...patch }; this.listeners.forEach(fn => fn());
  }
  capture(): Epoch { return { version: this.version, membership: this.state.membership?.membership_id ?? null }; }
  private valid(epoch: Epoch) {
    return this.active && epoch.version === this.version && epoch.membership === (this.state.membership?.membership_id ?? null);
  }
  isHost() {
    return !!this.state.userId && this.state.membership?.lobby?.owner_user_id === this.state.userId;
  }
  start() {
    if (this.active) return;
    this.active = true; this.version++; this.lifetime = new AbortController();
    if (this.state.membership?.membership_id) this.connect(this.capture());
    void this.sync();
  }
  stop() {
    this.active = false; this.version++; this.dirty = false; this.flight = undefined; this.pendingEnd = undefined;
    this.lifetime.abort(); this.connection?.abort(); this.connection = undefined;
  }
  private membership(next: CurrentLobby) {
    const previous = this.state.membership;
    if (previous?.membership_id === next.membership_id && previous.lobby && next.lobby &&
        BigInt(next.lobby.revision) < BigInt(previous.lobby.revision)) return;
    if (previous?.membership_id !== next.membership_id) {
      this.version++; this.connection?.abort(); this.connection = undefined; this.pendingEnd = undefined;
      this.update({ membership: next, queue: null, game: null, live: false, attempt: null, busy: null, error: null, gameBusy: null, gameError: null });
      if (next.membership_id) this.connect(this.capture());
    } else this.update({ membership: next });
  }
  applyQueue(view: QueueView, epoch: Epoch) {
    if (!this.valid(epoch) || view.membership_id !== epoch.membership || view.lobby_id !== this.state.membership?.lobby?.id) return;
    if (this.state.queue && BigInt(view.revision) < BigInt(this.state.queue.revision)) return;
    this.update({ queue: view });
  }
  /** Same rules as the queue: bound to the epoch's membership, never regresses the revision. */
  applyGame(snapshot: GameSnapshot, epoch: Epoch) {
    if (!this.valid(epoch) || snapshot.membership_id !== epoch.membership || snapshot.lobby_id !== this.state.membership?.lobby?.id) return;
    if (this.state.game && BigInt(snapshot.revision) < BigInt(this.state.game.revision)) return;
    const server = Date.parse(snapshot.server_time);
    this.update({ game: snapshot, clockOffset: Number.isNaN(server) ? this.state.clockOffset : server - Date.now() });
  }
  sync = (): Promise<void> => {
    if (!this.active) return Promise.resolve();
    this.dirty = true;
    if (this.flight) return this.flight;
    const signal = this.lifetime.signal;
    const work = this.synchronize(signal).finally(() => { if (this.flight === work) this.flight = undefined; });
    this.flight = work; return work;
  };
  private async synchronize(signal: AbortSignal) {
    let recoveries = 0;
    try {
      while (this.active && !signal.aborted && this.dirty) {
        this.dirty = false;
        const epoch = this.capture();
        const next = await this.deps.current(signal);
        if (!this.valid(epoch) || signal.aborted) continue;
        this.membership(next);
        const bound = this.capture();
        const identity = await this.deps.identity();
        if (!this.valid(bound) || signal.aborted) continue;
        this.update({ userId: identity });
        if (next.membership_id) {
          try {
            const readGame = this.deps.readGame;
            const [view, game] = await Promise.all([
              this.deps.read(next.membership_id, signal),
              readGame ? readGame(next.membership_id, signal) : Promise.resolve(undefined),
            ]);
            if (!signal.aborted) { this.applyQueue(view, bound); if (game) this.applyGame(game, bound); }
          } catch (error) {
            if (!this.valid(bound) || signal.aborted) continue;
            if (error instanceof QueueError && error.status === 403 && recoveries++ < 2) { this.dirty = true; continue; }
            throw error;
          }
        }
        if (this.valid(bound)) this.update({ syncError: null, loading: false });
      }
    } catch (error) {
      if (this.active && !signal.aborted) this.update({ loading: false, syncError: message(error), live: false });
    }
  }
  ended(reason: keyof typeof endedMessages, epoch: Epoch) {
    if (!this.valid(epoch)) return;
    // A host's leave closes the lobby, and the stream's end event can beat the DELETE response.
    if (this.leaving && this.leaving === epoch.membership) reason = "left";
    this.membership({ membership_id: null, lobby: null });
    this.update({ ended: endedMessages[reason], loading: false });
    // Discover any replacement generation without allowing the old read to restore it.
    void this.sync();
  }
  private connect(epoch: Epoch) {
    if (!epoch.membership) return;
    const connection = new AbortController(); this.connection = connection;
    void (async () => {
      let delay = 100 + Math.random() * 400;
      while (this.valid(epoch) && !connection.signal.aborted) {
        let expired = false;
        try {
          await this.deps.events(epoch.membership!, connection.signal, (event: LobbyEvent) => {
            if (!this.valid(epoch) || connection.signal.aborted) return;
            if (event.event === "membership_ended") { this.ended(event.data.reason, epoch); return; }
            if (event.event === "auth_expired") { expired = true; return; }
            this.update({ live: true }); delay = 250;
            void this.sync();
          });
          if (expired && this.valid(epoch)) await this.deps.refresh();
        } catch { /* Snapshot recovery distinguishes connection loss from departure. */ }
        if (!this.valid(epoch) || connection.signal.aborted) return;
        this.update({ live: false }); void this.sync();
        await new Promise<void>(resolve => {
          const finish = () => { clearTimeout(timer); connection.signal.removeEventListener("abort", finish); resolve(); };
          const timer = setTimeout(finish, delay); connection.signal.addEventListener("abort", finish, { once: true });
        });
        delay = Math.min(10000, delay * 2 * (0.75 + Math.random() * 0.5));
      }
    })();
  }
  async mutate(label: string, op: (membership: string) => Promise<Mutation>, epoch = this.capture()) {
    if (!this.valid(epoch) || !epoch.membership || this.state.busy) return false;
    this.update({ busy: label, error: null });
    try {
      const result = await op(epoch.membership);
      if (!this.valid(epoch)) return false;
      this.applyQueue(result.queue, epoch); return true;
    } catch (error) {
      if (!this.valid(epoch)) return false;
      this.update({ error: message(error) });
      await this.sync(); // Conflict/network uncertainty always recovers; never replay intent.
      throw error;
    } finally {
      if (this.valid(epoch)) {
        this.update({ busy: null });
        const pending = this.pendingEnd; this.pendingEnd = undefined;
        if (pending && !this.state.syncError) void this.advanceEntry(pending.entryId, "ended", pending.epoch).catch(() => {});
      }
    }
  }
  async add(url: string) {
    const videoUrl = youTubeVideoUrl(url);
    if (this.state.attempt || !videoUrl || this.state.busy) return;
    this.update({ attempt: { url: videoUrl, request_id: newRequestId() } });
    await this.retrySubmission();
  }
  async retrySubmission() {
    const attempt = this.state.attempt;
    if (!attempt || this.state.busy) return;
    const epoch = this.capture();
    try {
      const ok = await this.mutate("Adding music…", m => this.deps.submit(m, attempt), epoch);
      if (ok && this.valid(epoch)) this.update({ attempt: null });
    } catch (error) {
      if (!this.valid(epoch)) return;
      const uncertain = !(error instanceof QueueError) || error.status >= 500;
      this.update({ attempt: uncertain ? attempt : null, error: uncertain
        ? "The add may have completed. Retry this submission safely, or start a different submission."
        : message(error) });
    }
  }
  discardSubmission() { if (!this.state.busy) this.update({ attempt: null, error: null }); }
  /**
   * Runs one game write. Game writes have their own busy flag so the host's player
   * reporting a song end never waits on a queue edit. Failures resync and rethrow.
   */
  async gameMutate<T extends GameMutation>(label: string, op: (membership: string) => Promise<T>, epoch = this.capture()): Promise<T | undefined> {
    if (!this.valid(epoch) || !epoch.membership) return undefined;
    this.update({ gameBusy: label, gameError: null });
    try {
      const result = await op(epoch.membership);
      if (this.valid(epoch)) this.applyGame(result.game, epoch);
      return result;
    } catch (error) {
      // A phase or song that already moved on is routine (the ticker won the race): just resync.
      const routine = error instanceof QueueError && (error.code === "game_phase_changed" || error.code === "game_song_changed");
      if (this.valid(epoch)) { if (!routine) this.update({ gameError: message(error) }); await this.sync(); }
      throw error;
    } finally {
      if (this.valid(epoch) && this.state.gameBusy === label) this.update({ gameBusy: null });
    }
  }
  clearGameError() { this.update({ gameError: null }); }
  advanceEntry(entryId: string, reason: AdvanceReason, epoch = this.capture()) {
    if (!this.valid(epoch) || !this.isHost() || this.state.queue?.current?.id !== entryId) return Promise.resolve(false);
    if (this.state.busy && reason === "ended") { this.pendingEnd = { entryId, epoch }; return Promise.resolve(false); }
    return this.mutate("Advancing…", m => this.deps.advance(m, entryId, reason), epoch);
  }
  remove(entryId: string) {
    if (!this.isHost()) return Promise.resolve(false);
    return this.mutate("Removing…", m => this.deps.veto(m, entryId));
  }
  move(entryId: string, revision: string, index: number) {
    if (!this.isHost()) return Promise.resolve(false);
    return this.mutate("Moving…", m => this.deps.move(m, entryId, revision, index));
  }
  vote(entryId: string) {
    if (!this.state.userId || this.isHost()) return Promise.resolve(false);
    return this.mutate("Voting…", m => this.deps.vote(m, entryId));
  }
  policy(policy: Policy) {
    if (!this.isHost()) return Promise.resolve(false);
    if (policy.mode === "fixed" && (!Number.isInteger(policy.count) || policy.count < 1 || policy.count > 2147483647)) {
      return Promise.reject(new Error("Vote count must be an integer from 1 to 2147483647."));
    }
    return this.mutate("Updating policy…", m => this.deps.policy(m, policy));
  }
  async leave() {
    const epoch = this.capture();
    if (!epoch.membership || this.state.busy) return false;
    this.update({ busy: "Leaving…", error: null });
    this.leaving = epoch.membership;
    try {
      await this.deps.leave(epoch.membership);
      // The end event may already have closed this membership: that was this leave.
      if (!this.valid(epoch)) return this.state.ended === endedMessages.left;
      this.ended("left", epoch); return true;
    } catch (error) {
      if (this.valid(epoch)) { this.update({ error: message(error), busy: null }); await this.sync(); }
      return false;
    } finally {
      if (this.leaving === epoch.membership) this.leaving = null;
      if (this.state.busy === "Leaving…") this.update({ busy: null });
    }
  }
}
