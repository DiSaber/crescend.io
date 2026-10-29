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

export default function ReportBugLink() {
  return (
    <a
      href={NEW_ISSUE_URL}
      target="_blank"
      rel="noopener noreferrer"
      onClick={(e) => {
        e.preventDefault();
        window.open(buildReportUrl(), "_blank", "noopener,noreferrer");
      }}
      className="fixed bottom-2 right-3 z-50 text-xs text-zinc-400 hover:text-[#FF0048] underline-offset-2 hover:underline transition"
    >
      Report a bug
    </a>
  );
}
