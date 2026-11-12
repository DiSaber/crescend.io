"use client";

import { useEffect, useState, type ReactNode } from "react";
import { login, getToken, subscribeAuth } from "../lib/auth";

type Status = "checking" | "signed-in" | "signed-out" | "error";

/**
 * Renders children only after the refresh cookie yields an access token. With
 * `joinFirst` (lobby pages) a signed-out visitor is offered the code first and Google
 * second, since only hosts need Google.
 */
export default function SignInGate({ returnTo, message = "Sign in to continue.", joinFirst = false, children }: {
  returnTo: string; message?: string; joinFirst?: boolean; children: ReactNode;
}) {
  const [status, setStatus] = useState<Status>("checking");

  useEffect(() => {
    let active = true;
    const unsubscribe = subscribeAuth(() => { if (active) setStatus("signed-out"); });
    getToken()
      .then((token) => { if (active) setStatus(token ? "signed-in" : "signed-out"); })
      .catch(() => { if (active) setStatus("error"); });
    return () => { active = false; unsubscribe(); };
  }, []);

  if (status === "signed-in") return <>{children}</>;

  return (
    <div className="w-full max-w-md bg-zinc-900 p-8 rounded-xl border border-zinc-800 text-center">
      {status === "checking" && <p className="text-zinc-400">Checking sign-in…</p>}
      {status === "error" && <p className="text-red-400 mb-4">Couldn&apos;t reach the server. Is the backend running?</p>}
      {status !== "checking" && joinFirst && (
        <>
          <p className="text-zinc-300 mb-6">{message}</p>
          <a href="/join/" className="flex w-full min-h-12 items-center justify-center p-3 rounded-lg bg-purple-600 hover:bg-purple-700 transition font-semibold">
            Join with a code
          </a>
          <button
            onClick={() => login(returnTo)}
            className="mt-3 w-full min-h-12 p-3 rounded-lg border border-zinc-600 hover:border-zinc-400 transition font-semibold text-zinc-200"
          >
            Host with Google
          </button>
        </>
      )}
      {status !== "checking" && !joinFirst && (
        <>
          <p className="text-zinc-300 mb-6">{message}</p>
          <button
            onClick={() => login(returnTo)}
            className="w-full p-3 rounded-lg bg-purple-600 hover:bg-purple-700 transition font-semibold"
          >
            Sign in with Google
          </button>
          <a href="/join/" className="mt-4 inline-flex min-h-11 items-center text-sm text-zinc-300 underline hover:text-white">
            Join a session with just a nickname
          </a>
        </>
      )}
    </div>
  );
}
