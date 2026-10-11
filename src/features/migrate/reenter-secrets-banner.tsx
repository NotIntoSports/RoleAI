import type { LegacyMigrationStatus } from "../../generated/bindings";
import { t } from "../../i18n";

interface ReenterSecretsBannerProps {
  status: LegacyMigrationStatus | null;
  onOpenProviders: () => void;
}

export function ReenterSecretsBanner({ status, onOpenProviders }: ReenterSecretsBannerProps) {
  if (!status?.reenterSecrets) {
    return null;
  }
  return (
    <p className="reenter-secrets-banner" role="status">
      {t("migrate.banner.title")}
      <button type="button" className="button-ghost" onClick={onOpenProviders}>{t("migrate.banner.action")}</button>
    </p>
  );
}
