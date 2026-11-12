"use client";

import { Suspense, useEffect, useState, type FormEvent } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { account, guestSignIn, lastGuestNickname, login } from "../lib/auth";
import { joinLobby, nicknameProblem, NICKNAME_MAX } from "../lib/lobby";

// Joining needs only the code and a nickname (docs/guest-join.md). Signed-in Google
// users join as themselves; everyone else becomes this browser's guest account.
function JoinSession({ initialCode }: { initialCode: string }) {
  const router = useRouter();
  const [code, setCode] = useState(initialCode);
  const [nickname, setNickname] = useState("");
  const [signedIn, setSignedIn] = useState<"checking" | "google" | "guest" | "no">("checking");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  useEffect(() => {
    let active = true;
    // Offer the last nickname: the account's when signed in, else this browser's guest one.
    account()
      .catch(() => undefined)
      .then(async (me) => {
        const last = me ? me.last_nickname ?? undefined : await lastGuestNickname();
        if (!active) return;
        setSignedIn(me ? (me.guest ? "guest" : "google") : "no");
        if (last) setNickname((current) => current || last);
      });
    return () => { active = false; };
  }, []);

  async function join(e: FormEvent) {
    e.preventDefault();
    const problem = nicknameProblem(nickname);
    if (problem) { setError(problem); return; }
    setBusy(true);
    setError(undefined);
    try {
      if (signedIn === "no" || signedIn === "checking") await guestSignIn();
      await joinLobby(code, nickname.trim());
      router.push("/lobby/");
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  }

  const returnTo = code ? `/join/?code=${code}` : "/join/";
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
            onChange={(e) => setCode(e.target.value.toUpperCase())}
            placeholder="e.g. A1B2C3"
            maxLength={12}
            autoComplete="off"
            autoFocus={!initialCode}
            className="w-full p-3 rounded-lg bg-zinc-800 border border-zinc-700 text-white text-lg text-center tracking-[0.3em] placeholder-zinc-500 placeholder:tracking-normal focus:outline-none focus:border-purple-500"
          />
        </label>

        <label className="flex flex-col gap-1 text-sm text-zinc-400">
          Your nickname
          <input
            type="text"
            name="nickname"
            value={nickname}
            onChange={(e) => setNickname(e.target.value)}
            placeholder="What should everyone call you?"
            maxLength={NICKNAME_MAX * 2}
            autoComplete="nickname"
            autoFocus={!!initialCode}
            className="w-full p-3 rounded-lg bg-zinc-800 border border-zinc-700 text-white text-lg placeholder-zinc-500 focus:outline-none focus:border-purple-500"
          />
        </label>

        <button
          type="submit"
          disabled={busy || code.trim().length === 0 || nickname.trim().length === 0}
          className="w-full min-h-12 p-3 rounded-lg bg-purple-600 hover:bg-purple-700 disabled:opacity-50 transition font-semibold"
        >
          {busy ? "Joining…" : "Join Session"}
        </button>
        {error && <p role="alert" className="text-red-400 text-center">{error}</p>}
        <p className="text-xs text-zinc-400 text-center">
          {signedIn === "google"
            ? "You're signed in with Google."
            : <>No account needed. Have one? <button type="button" onClick={() => login(returnTo)} className="inline-flex min-h-11 items-center underline hover:text-zinc-300">Sign in with Google</button></>}
        </p>
      </div>
    </form>
  );
}

// A scanned QR code opens /join/?code=XXXXXX.
function JoinFromLink() {
  const code = (useSearchParams().get("code") ?? "").toUpperCase().replace(/[^A-Z0-9]/g, "").slice(0, 12);
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
