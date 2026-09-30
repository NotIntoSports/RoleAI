import { routeLabel, type RouteId } from "../app/routes";
import { t, useT } from "../i18n";

export interface PageShellProps {
  id: RouteId;
}

export function PageShell({ id }: PageShellProps) {
  useT();
  const label = routeLabel(id);
  const headingId = `page-heading-${id}`;

  return (
    <header className="page-header" role="region" aria-labelledby={headingId}>
      <div><h1 id={headingId}>{label}</h1><p>{t(`app.routes.descriptions.${id}`)}</p></div>
    </header>
  );
}
