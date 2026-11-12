/** Browser auth lifecycle; injected transport/lock makes races reproducible in tests. */
export class SignedOutError extends Error {
  constructor() { super("Sign in again to continue."); }
}
export type StopReason = "signed-out" | "uncertain";
type Environment = {
  fetch: typeof fetch;
  lock: <T>(fn: () => Promise<T>) => Promise<T>;
  publish: (reason: StopReason) => void;
  isStopped?: () => boolean;
};
export class AuthCoordinator {
  private env: Environment;
  private accessToken: string | undefined;
  private refreshing: Promise<string | undefined> | undefined;
  private stopped = false;
  private epoch = 0;
  private listeners = new Set<() => void>();
  constructor(env: Environment) { this.env = env; }
  subscribe = (fn: () => void) => { this.listeners.add(fn); return () => { this.listeners.delete(fn); }; };
  token() { return this.stopped ? undefined : this.accessToken; }
  stop(reason: StopReason, broadcast = false) {
    this.stopped = true;
    this.epoch++;
    this.accessToken = undefined;
    if (broadcast) this.env.publish(reason);
    this.listeners.forEach(fn => fn());
  }
  private allowed() {
    if (this.env.isStopped?.() && !this.stopped) this.stop("signed-out");
    return !this.stopped;
  }
  refresh(): Promise<string | undefined> {
    if (!this.allowed()) return Promise.resolve(undefined);
    if (this.refreshing) return this.refreshing;
    const epoch = this.epoch;
    this.refreshing = this.env.lock(async () => {
      if (!this.allowed() || epoch !== this.epoch) return undefined;
      try {
        const response = await this.env.fetch("/api/auth/refresh", {
          method: "POST", credentials: "same-origin", headers: { "X-CSRF-Protection": "1" },
        });
        if (!this.allowed() || epoch !== this.epoch) return undefined;
        if (response.status === 401) { this.stop("signed-out", true); return undefined; }
        if (!response.ok) throw new Error(`Refresh failed: ${response.status}. Sign in again.`);
        const body = await response.json();
        if (!this.allowed() || epoch !== this.epoch) return undefined;
        if (typeof body.access_token !== "string") throw new Error("Invalid refresh response. Sign in again.");
        this.accessToken = body.access_token;
        return this.accessToken;
      } catch (error) {
        // The server may have consumed the cookie. Never automatically replay it.
        if (epoch === this.epoch) this.stop("uncertain", true);
        throw error;
      }
    }).finally(() => { this.refreshing = undefined; });
    return this.refreshing;
  }
  async getToken() {
    if (!this.allowed()) return undefined;
    if (this.accessToken) {
      try {
        const { exp } = JSON.parse(atob(this.accessToken.split('.')[1].replace(/-/g, '+').replace(/_/g, '/')));
        if (typeof exp === "number" && exp * 1000 <= Date.now() + 15000) return this.refresh();
      } catch { /* Opaque tokens are still authorized by the server. */ }
      return this.accessToken;
    }
    return this.refresh();
  }
  /**
   * Deliberately sign in as a guest (docs/guest-join.md): like a new Google login it
   * clears an earlier stop, then takes the shared lock so it never races a refresh.
   * The server reuses this browser's guest account when its guest cookie is present.
   */
  async guestSignIn() {
    this.stopped = false;
    this.epoch++;
    this.accessToken = undefined;
    const epoch = this.epoch;
    const token = await this.env.lock(async () => {
      const response = await this.env.fetch("/api/auth/guest", {
        method: "POST", credentials: "same-origin", headers: { "X-CSRF-Protection": "1" },
      });
      if (response.status === 429) throw new Error("Too many new guests from this network right now. Try again later.");
      if (!response.ok) throw new Error(`Couldn't join as a guest (${response.status}). Try again.`);
      const body = await response.json();
      if (typeof body.access_token !== "string") throw new Error("Invalid sign-in response. Try again.");
      return body.access_token as string;
    });
    if (this.stopped || epoch !== this.epoch) throw new SignedOutError();
    this.accessToken = token;
    return token;
  }
  async logout() {
    this.stop("signed-out", true);
    await this.env.lock(async () => {
      const response = await this.env.fetch("/api/auth/logout", {
        method: "POST", credentials: "same-origin", headers: { "X-CSRF-Protection": "1" },
      });
      if (response.status !== 204) throw new Error(`Logout failed: ${response.status}. Retry signing out.`);
    });
  }
  async apiFetch(path: string, init: RequestInit = {}) {
    const epoch = this.epoch;
    let token = await this.getToken();
    const active = () => {
      if (!this.allowed() || epoch !== this.epoch) throw new SignedOutError();
      init.signal?.throwIfAborted();
    };
    const send = (value: string) => {
      active(); const headers = new Headers(init.headers); headers.set("Authorization", `Bearer ${value}`);
      return this.env.fetch(path, { ...init, cache: "no-store", headers });
    };
    if (!token) throw new SignedOutError();
    let response = await send(token); active();
    if (response.status === 401) {
      // A concurrent request may already have refreshed this token.
      token = this.accessToken !== token ? this.accessToken : await this.refresh();
      if (!token) throw new SignedOutError();
      response = await send(token); active();
      if (response.status === 401) { this.stop("signed-out", true); throw new SignedOutError(); }
    }
    return response;
  }
}
