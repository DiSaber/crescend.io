This is a [Next.js](https://nextjs.org) project bootstrapped with [`create-next-app`](https://nextjs.org/docs/app/api-reference/cli/create-next-app).

## Getting Started

First, run the development server:

```bash
npm run dev
# or
yarn dev
# or
pnpm dev
# or
bun dev
```

Open [http://localhost:3000](http://localhost:3000) with your browser to see the result.

You can start editing the page by modifying `app/page.tsx`. The page auto-updates as you edit the file.

This project uses [`next/font`](https://nextjs.org/docs/app/building-your-application/optimizing/fonts) to automatically optimize and load [Geist](https://vercel.com/font), a new font family for Vercel.

## Learn More

To learn more about Next.js, take a look at the following resources:

- [Next.js Documentation](https://nextjs.org/docs) - learn about Next.js features and API.
- [Learn Next.js](https://nextjs.org/learn) - an interactive Next.js tutorial.

You can check out [the Next.js GitHub repository](https://github.com/vercel/next.js) - your feedback and contributions are welcome!

## Deploy on Vercel

The easiest way to deploy your Next.js app is to use the [Vercel Platform](https://vercel.com/new?utm_medium=default-template&filter=next.js&utm_source=create-next-app&utm_campaign=create-next-app-readme) from the creators of Next.js.

Check out our [Next.js deployment documentation](https://nextjs.org/docs/app/building-your-application/deploying) for more details.


## Shared queue lobby

`/lobby/` reads the persistent queue from the API. Members can search saved/starter
songs or paste a YouTube link; search results use the same guarded submission flow.
The host QR code opens the join page with its code prefilled and preserved through
sign-in. All members can add YouTube links;
only the host plays audio and can skip, veto or move upcoming occurrences. Guests
vote to skip; the host can apply majority or fixed-count voting policy. The UI
preserves server order and reloads membership/queue after SSE invalidations.

An uncertain add keeps its submission ID. Use **Retry submission** to safely retry
that same intent. **Start a different submission** abandons that retry; the previous
song may already have been added. This pending intent is held in memory, so recover
and check the shared queue before submitting again after a page reload.

Playback errors do not skip automatically. The host can press **Play**, **Retry
player**, or explicitly **Skip unplayable song**. Use one host playback tab; device
election and synchronized guest audio are outside this feature.

Refresh and logout share an origin-wide Web Lock. Logout/recovery intent is shared
through BroadcastChannel and a local-storage stop flag (never credentials). A lost
refresh response stops automatic refreshing and requires deliberate Google sign-in.
Use a browser with Web Locks over HTTPS or localhost.

### Verification

With Node >=22.18 and dependencies installed:

```bash
npm test
npm run lint
npm run build
```

The test runner uses native TypeScript stripping; no test framework dependency is
needed. When Turbopack cannot bind its build-worker port in a restricted environment,
`npm run build -- --webpack` uses Next.js's alternative supported bundler. Both build
paths require access to the Google fonts already configured by the application.

Before accepting the feature, use one host and two guests to verify shared additions,
votes, reordering, reconnects, guest transfer between lobbies, host confirmation
before closing an old lobby, and logout across tabs. Check playback errors on the
real host device. Unit tests mock the service; they do not establish backend vote
arithmetic, real OAuth, provider playback or live convergence timing.
