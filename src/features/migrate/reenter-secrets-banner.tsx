import type { LegacyMigrationStatus } from "../../generated/bindings";
import { t } from "../../i18n";

interface ReenterSecretsBannerProps {
  status: LegacyMigrationStatus | null;
}

export function ReenterSecretsBanner({ status }: ReenterSecretsBannerProps) {
  if (!status?.reenterSecrets) {
    return null;
  }
  return (
    <p className="reenter-secrets-banner" role="status">
      {t("migrate.banner.title")}
      <a href="#/services">{t("migrate.banner.action")}</a>
    </p>
  );
}
