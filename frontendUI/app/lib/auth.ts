// Tokens stay in memory. Only logout/recovery intent is shared across tabs.
import { AuthCoordinator, type StopReason } from "./auth-coordinator.ts";
export { SignedOutError } from "./auth-coordinator.ts";
const STOP_KEY = "crescend-auth-stopped";
let coordinator: AuthCoordinator | undefined;
function auth() {
  if (coordinator) return coordinator;
  let channel: BroadcastChannel | undefined;
  if (typeof window !== "undefined" && typeof BroadcastChannel !== "undefined") {
    channel = new BroadcastChannel("crescend-auth");
  }
  const stopped = () => {
    try { return typeof window !== "undefined" && localStorage.getItem(STOP_KEY) !== null; }
    catch { return false; }
  };
  coordinator = new AuthCoordinator({
    fetch: (...args) => fetch(...args),
    lock: async <T>(fn: () => Promise<T>): Promise<T> => {
      if (typeof navigator !== "undefined" && navigator.locks) return await navigator.locks.request("crescend-auth", fn);
      // Rotating credentials cannot safely be shared by tabs without origin-wide locking.
      return Promise.reject(new Error("This browser needs Web Locks for secure sign-in. Use a current browser over HTTPS or localhost."));
    },
    isStopped: stopped,
    publish: (reason: StopReason) => {
      try { localStorage.setItem(STOP_KEY, reason); } catch { /* BroadcastChannel still notifies open tabs. */ }
      channel?.postMessage(reason);
    },
  });
  const instance = coordinator;
  if (channel) channel.onmessage = e => {
    if (e.data === "signed-out" || e.data === "uncertain") instance.stop(e.data);
  };
  if (typeof window !== "undefined") window.addEventListener("storage", e => {
    if (e.key === STOP_KEY && e.newValue) instance.stop("signed-out");
  });
  return instance;
}
export const refresh = () => auth().refresh();
export const getToken = () => auth().getToken();
export const logout = () => auth().logout();
export const subscribeAuth = (fn: () => void) => auth().subscribe(fn);
export const apiFetch = (path: string, init: RequestInit = {}) => auth().apiFetch(path, init);
/** UI hint only; backend still authorizes all requests. */
export async function currentUserId(): Promise<string | undefined> {
  const token = await getToken();
  if (!token) return undefined;
  try {
    const payload = JSON.parse(atob(token.split(".")[1].replace(/-/g, "+").replace(/_/g, "/")));
    return typeof payload.sub === "string" && /^\d+$/.test(payload.sub) ? payload.sub : undefined;
  } catch { return undefined; }
}
export function login(returnTo: string) {
  try { localStorage.removeItem(STOP_KEY); } catch { /* New page starts a new coordinator. */ }
  window.location.assign("/api/auth/google/start?" + new URLSearchParams({ return_to: returnTo }));
}
