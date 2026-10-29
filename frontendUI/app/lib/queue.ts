// Shared queue API client, following docs/queue-api.md.
import { apiFetch } from "./auth";
import type { VideoMetadata } from "./lobby";

export type QueueEntry = {
  id: string;
  video: VideoMetadata;
  submitted_by_user_id: string;
  submitted_at: string;
  active_vote_count: number;
  has_voted: boolean;
};

export type VotingView = {
  mode: "majority" | "fixed";
  fixed_count: number | null;
  active_guest_count: number;
  effective_threshold: number | null;
};

export type QueueView = {
  membership_id: string;
  lobby_id: string;
  /** Decimal string; compare with BigInt. */
  revision: string;
  current: QueueEntry | null;
  upcoming: QueueEntry[];
  voting: VotingView;
};

export type AdvanceReason = "ended" | "skip" | "playback_error_skip";

/** A non-2xx queue response, keeping the stable `code` for branching. */
export class QueueError extends Error {
  constructor(
    readonly status: number,
    readonly code: string | undefined,
    message: string,
  ) {
    super(message);
  }
}

async function check<T>(response: Response): Promise<T> {
  if (response.ok) return response.json();
  let code: string | undefined;
  let message = `HTTP ${response.status}`;
  try {
    const body = await response.json();
    code = body.code;
    message = body.error ?? message;
  } catch {}
  throw new QueueError(response.status, code, message);
}

const base = (membershipId: string) =>
  `/api/lobbies/memberships/${encodeURIComponent(membershipId)}/queue`;
const entry = (membershipId: string, entryId: string) =>
  `${base(membershipId)}/entries/${encodeURIComponent(entryId)}`;
const json = (body: unknown): RequestInit => ({
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify(body),
});

type Mutation = { changed: boolean; queue: QueueView };

/** 32 lowercase hex characters; keep one per submission attempt across retries. */
export function newRequestId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/** True when `a` is a strictly newer revision than `b` (or `b` is unknown). */
export function newer(a: string, b: string | undefined): boolean {
  return b === undefined || BigInt(a) > BigInt(b);
}

export async function getQueue(membershipId: string): Promise<QueueView> {
  return check(await apiFetch(base(membershipId)));
}

export async function submitEntry(membershipId: string, url: string, requestId: string) {
  return check<Mutation & { entry_id: string; active: boolean }>(
    await apiFetch(`${base(membershipId)}/entries`, json({ url, request_id: requestId })),
  );
}

export async function advance(membershipId: string, expectedEntryId: string, reason: AdvanceReason) {
  return check<Mutation>(
    await apiFetch(`${base(membershipId)}/advance`, json({ expected_entry_id: expectedEntryId, reason })),
  );
}

export async function vote(membershipId: string, entryId: string) {
  return check<Mutation>(await apiFetch(`${entry(membershipId, entryId)}/votes`, { method: "POST" }));
}

export async function veto(membershipId: string, entryId: string) {
  return check<Mutation>(await apiFetch(entry(membershipId, entryId), { method: "DELETE" }));
}

export async function moveEntry(membershipId: string, entryId: string, expectedRevision: string, toIndex: number) {
  return check<Mutation>(
    await apiFetch(`${entry(membershipId, entryId)}/move`, json({ expected_revision: expectedRevision, to_index: toIndex })),
  );
}

export async function setVotingPolicy(membershipId: string, policy: { mode: "majority" } | { mode: "fixed"; count: number }) {
  return check<Mutation>(
    await apiFetch(`${base(membershipId)}/voting-policy`, { ...json(policy), method: "PUT" }),
  );
}
