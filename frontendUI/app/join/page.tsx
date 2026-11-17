"use client";

import { Suspense, useEffect, useRef, useState, type FormEvent } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { account, currentUserId, guestSignIn, lastGuestNickname, login, logout, type Account } from "../lib/auth";
import { CODE_HINT } from "../lib/auth-coordinator";
import {
  getCurrent, JOIN_NOTICE_KEY, joinLobby, nicknameProblem, normalizeCode, normalizeNickname, NICKNAME_MAX,
  type CurrentLobby,
} from "../lib/lobby";

const field = "w-full p-3 rounded-lg bg-zinc-800 border border-zinc-700 text-white text-lg placeholder-zinc-500 focus:outline-none focus:border-purple-500";
const primary = "w-full min-h-12 p-3 rounded-lg bg-purple-600 hover:bg-purple-700 disabled:opacity-50 transition font-semibold";
const secondary = "w-full min-h-12 p-3 rounded-lg border border-zinc-600 hover:border-zinc-400 disabled:opacity-50 transition font-semibold text-zinc-200";

// Joining needs only the code and a nickname (docs/guest-join.md). Signed-in Google
// users choose: join as themselves, or sign out here and join as a guest. The host of
// this very session is told so instead of being renamed.
function JoinSession({ initialCode }: { initialCode: string }) {
  const router = useRouter();
  const [code, setCode] = useState(initialCode);
  const [nickname, setNickname] = useState("");
  // Prefill only an empty box nobody has typed in: a late answer must not mix with
  // what the user typed ("Cee" + "Cee" was "CeeCee").
  const touched = useRef(false);
  const [me, setMe] = useState<Account | null>();
  const [current, setCurrent] = useState<CurrentLobby | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let active = true;
    // Offer the last nickname: the account's when signed in, else this browser's guest one.
    account()
      .catch(() => undefined)
      .then(async (value) => {
        const [last, lobby] = await Promise.all([
          value ? value.last_nickname ?? undefined : lastGuestNickname(),
          value ? getCurrent().catch(() => null) : null,
        ]);
        if (!active) return;
        setMe(value ?? null);
        setCurrent(lobby);
        if (last && !touched.current) setNickname((now) => now === "" ? last : now);
      });
    return () => { active = false; };
  }, []);

  const google = !!me && !me.guest;
  const lobby = current?.lobby ?? null;
  const hosting = google && !!lobby && lobby.owner_user_id === me!.user_id ? lobby : null;
  const hostingThis = hosting && hosting.join_code === code ? hosting : null;
  const hostingOther = hosting && hosting.join_code !== code ? hosting : null;

  function check(): boolean {
    if (code.length !== 6) { setError(CODE_HINT); return false; }
    const problem = nicknameProblem(nickname);
    if (problem) { setError(problem); return false; }
    return true;
  }

  async function finish(asGuest: boolean) {
    const wanted = normalizeNickname(nickname);
    const view = await joinLobby(code, wanted);
    const self = asGuest ? await currentUserId() : me?.user_id;
    const mine = view.lobby?.members.find((m) => m.user_id === self);
    try {
      if (mine && mine.nickname !== wanted) sessionStorage.setItem(JOIN_NOTICE_KEY, JSON.stringify({ wanted, got: mine.nickname }));
      else sessionStorage.removeItem(JOIN_NOTICE_KEY);
    } catch { /* The notice is a nicety. */ }
    router.push("/lobby/");
  }

  async function run(work: () => Promise<void>) {
    setBusy(true);
    setError(undefined);
    try {
      await work();
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  }

  function join(e: FormEvent) {
    e.preventDefault();
    // Every new action starts clean: an earlier refusal no longer applies.
    setError(undefined);
    if (!check()) return;
    void run(async () => {
      if (!me) await guestSignIn(code);
      await finish(!google);
    });
  }

  function joinAsGuest() {
    setError(undefined);
    if (!check()) return;
    if (hostingOther && !window.confirm(
      `You're hosting session ${hostingOther.join_code}. Joining as a guest signs this browser out of Google. ` +
      "That session keeps running, but you can't control it from here until you sign in again. Continue?",
    )) return;
    void run(async () => {
      await logout();
      await guestSignIn(code);
      await finish(true);
    });
  }

  if (me === undefined) return <p role="status" className="text-zinc-400">Checking sign-in…</p>;

  if (hostingThis) {
    const link = `${window.location.origin}/join/?code=${hostingThis.join_code}`;
    return (
      <div className="w-full max-w-md bg-zinc-900 p-6 sm:p-8 rounded-xl border border-zinc-800 text-center">
        <h2 className="text-3xl font-bold mb-4">You&apos;re the host</h2>
        <p className="text-zinc-300 mb-6">
          You&apos;re hosting this session. To play as a guest too, open this link in a private window or on another device.
        </p>
        <button type="button" onClick={() => router.push("/lobby/")} className={primary}>Go to my lobby</button>
        <label className="mt-6 flex flex-col gap-1 text-left text-sm text-zinc-400">
          Join link
          <div className="flex gap-2">
            <input readOnly value={link} onFocus={(e) => e.currentTarget.select()} className="min-w-0 flex-1 rounded-lg bg-zinc-800 border border-zinc-700 p-3 text-sm text-white" />
            <button
              type="button"
              onClick={() => { void navigator.clipboard?.writeText(link).then(() => setCopied(true), () => setCopied(false)); }}
              className="min-h-11 min-w-11 shrink-0 rounded-lg bg-zinc-700 px-4 text-sm font-semibold hover:bg-zinc-600"
            >
              {copied ? "Copied" : "Copy"}
            </button>
          </div>
        </label>
      </div>
    );
  }

  const returnTo = code ? `/join/?code=${code}` : "/join/";
  const shown = normalizeNickname(nickname);
  return (
    <form onSubmit={join} className="w-full max-w-md bg-zinc-900 p-6 sm:p-8 rounded-xl border border-zinc-800">
      <h2 className="text-3xl font-bold text-center mb-6">Join a Session</h2>

      <div className="flex flex-col gap-4">
        <label className="flex flex-col gap-1 text-sm text-zinc-400">
          Session code
          <input
            type="text"
            name="code"
            value={code}
            onChange={(e) => { setCode(normalizeCode(e.target.value)); setError(undefined); }}
            placeholder="e.g. A1B2C3"
            maxLength={200}
            autoComplete="off"
            autoCapitalize="characters"
            spellCheck={false}
            aria-describedby="code-hint"
            autoFocus={!initialCode}
            className={`${field} text-center tracking-[0.3em] placeholder:tracking-normal`}
          />
          <span id="code-hint" className="text-xs text-zinc-500">Six letters or numbers. Spaces and dashes are fine.</span>
        </label>

        <label className="flex flex-col gap-1 text-sm text-zinc-400">
          Your nickname
          <input
            type="text"
            name="nickname"
            value={nickname}
            onChange={(e) => { touched.current = true; setNickname(e.target.value); setError(undefined); }}
            placeholder="What should everyone call you?"
            maxLength={NICKNAME_MAX * 12}
            autoComplete="nickname"
            autoFocus={!!initialCode}
            className={field}
          />
        </label>

        {google ? (
          <>
            <button type="submit" disabled={busy || !code || !shown} className={primary}>
              {busy ? "Joining…" : shown ? <>Join as <span className="break-all">{shown}</span> with Google</> : "Join with Google"}
            </button>
            <button type="button" onClick={joinAsGuest} disabled={busy || !code || !shown} className={secondary}>
              Join as a guest instead
            </button>
          </>
        ) : (
          <button type="submit" disabled={busy || !code || !shown} className={primary}>
            {busy ? "Joining…" : "Join Session"}
          </button>
        )}
        {error && <p role="alert" className="text-red-400 text-center">{error}</p>}
        <p className="text-xs text-zinc-400 text-center">
          {google
            ? "You're signed in with Google. Joining as a guest signs this browser out of Google first."
            : me?.guest
              ? "You're joining as this browser's guest."
              : <>No account needed. Have one? <button type="button" onClick={() => { setError(undefined); login(returnTo); }} className="inline-flex min-h-11 items-center underline hover:text-zinc-300">Sign in with Google</button></>}
        </p>
      </div>
    </form>
  );
}

// A scanned QR code opens /join/?code=XXXXXX.
function JoinFromLink() {
  const code = normalizeCode(useSearchParams().get("code") ?? "");
  return <JoinSession initialCode={code} />;
}

export default function JoinSessionPage() {
  return (
    <div className="flex flex-col items-center justify-center min-h-screen bg-gradient-to-b from-black to-[#1a0b29] text-white px-4">
      <Suspense>
        <JoinFromLink />
      </Suspense>
    </div>
  );
}
