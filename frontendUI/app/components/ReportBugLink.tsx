"use client";

const NEW_ISSUE_URL = "https://github.com/DiSaber/crescend.io/issues/new";

// Built at click time: this is a static export, so the page path and browser
// are only known on the client. Pathname only (no query or hash) keeps any
// tokens out of the URL; the user agent is the only other thing included.
function buildReportUrl(): string {
  const path = window.location.pathname;
  const params = new URLSearchParams({
    template: "bug_report.md",
    labels: "bug",
    title: `[Bug] ${path}: `,
    body: `**Page:** ${path}\n**Browser:** ${navigator.userAgent}\n\n**What happened?**\n\n\n**What did you expect to happen?**\n\n\n**Steps to reproduce**\n1.\n2.\n3.\n`,
  });
  return `${NEW_ISSUE_URL}?${params.toString()}`;
}

// In the page flow on phones and tablets, below the content, so it never covers lobby
// controls while scrolling; pinned to the corner only on wide screens with free margins.
export default function ReportBugLink() {
  return (
    <footer className="w-full flex justify-end bg-[#1a0b29] px-3 xl:fixed xl:bottom-2 xl:right-3 xl:w-auto xl:bg-transparent xl:px-0 z-50">
      <a
        href={NEW_ISSUE_URL}
        target="_blank"
        rel="noopener noreferrer"
        onClick={(e) => {
          e.preventDefault();
          window.open(buildReportUrl(), "_blank", "noopener,noreferrer");
        }}
        className="min-h-11 inline-flex items-center text-xs text-zinc-400 hover:text-[#FF0048] underline-offset-2 hover:underline transition xl:min-h-0"
      >
        Report a bug
      </a>
    </footer>
  );
}
