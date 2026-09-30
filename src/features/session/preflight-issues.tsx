import { Link } from "wouter";
import type { PreflightIssue } from "../../generated/bindings";
import { t, useT } from "../../i18n";

const ISSUE_CODES = [
  "SESSION_ROLE_REQUIRED",
  "SESSION_ROUTE_REQUIRED",
  "SESSION_STAGE_INCOMPLETE",
  "SESSION_CREDENTIAL_MISSING",
  "SESSION_DATABASE_UNAVAILABLE",
] as const;

type KnownIssueCode = (typeof ISSUE_CODES)[number];

function issueHrefs(): Record<KnownIssueCode, { href: string }> {
  return {
    SESSION_ROLE_REQUIRED: { href: "/settings?category=roles" },
    SESSION_ROUTE_REQUIRED: { href: "/services?category=routes" },
    SESSION_STAGE_INCOMPLETE: { href: "/services?category=routes" },
    SESSION_CREDENTIAL_MISSING: { href: "/services?category=providers" },
    SESSION_DATABASE_UNAVAILABLE: { href: "/settings" },
  };
}

export function describeIssue(issue: PreflightIssue) {
  const hrefs = issueHrefs();
  const known = (ISSUE_CODES as readonly string[]).includes(issue.code)
    ? (issue.code as KnownIssueCode)
    : null;
  if (!known) return { text: t("session.preflight.GENERIC"), href: "/settings", action: t("session.preflight.GENERIC_ACTION") };
  return { text: t(`session.preflight.${known}`), href: hrefs[known].href, action: t(`session.preflight.${known}_ACTION`) };
}

export function PreflightIssues({ issues }: { issues: PreflightIssue[] }) {
  useT();
  if (!issues.length) return null;
  return <div className="services-message" role="status" aria-label={t("session.preflight.region")}>
    {issues.map((issue, index) => {
      const help = describeIssue(issue);
      return <div key={`${issue.code}-${index}`}><span>{help.text} </span><Link href={help.href}>{help.action}</Link></div>;
    })}
  </div>;
}
