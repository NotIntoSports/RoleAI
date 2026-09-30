import { useCallback, useEffect, useState } from "react";
import { ClipboardList, History } from "lucide-react";

import * as api from "../../api/commands";
import { EmptyState } from "../../components/empty-state";
import { t, useT } from "../../i18n";
import type { PracticeReportSummary } from "../../generated/bindings";
import { PracticeHistorySection } from "../../features/practice/practice-history";
import { PracticeReportView } from "../../features/practice/practice-report";
import { PracticeWizard } from "../../features/practice/practice-wizard";
import { PageShell } from "../page-shell";

export function PracticePage() {
  useT();
  // 报告区默认展示最近一次训练；历史列表（E09）可切换任意一份报告。
  const [reports, setReports] = useState<PracticeReportSummary[] | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);

  const loadReports = useCallback(async () => {
    try {
      const result = await api.listPracticeReports();
      if (result.ok) {
        setReports(result.data);
        setSelectedSessionId(result.data[0]?.sessionId ?? null);
        setLoadFailed(false);
      } else {
        setReports(null);
        setLoadFailed(true);
      }
    } catch {
      setReports(null);
      setLoadFailed(true);
    }
  }, []);

  useEffect(() => {
    void loadReports();
  }, [loadReports]);

  const loading = reports === null && !loadFailed;
  const showEmpty = loadFailed || (reports !== null && reports.length === 0);
  const hasReports = !!reports && reports.length > 0;
  const isLatest = hasReports && !!selectedSessionId && reports[0].sessionId === selectedSessionId;

  return (
    <>
      <PageShell id="practice" />
      <PracticeWizard />
      <section className="service-panel practice-report-section" aria-labelledby="practice-report-heading">
        <div className="library-heading">
          <h2 id="practice-report-heading">
            <ClipboardList size={16} aria-hidden="true" />
            {t("practice.page.reportHeading")}
          </h2>
          {hasReports && <span className="muted">{isLatest ? t("practice.page.latest") : t("practice.page.fromHistory")}</span>}
        </div>
        {loading && <p className="muted" role="status">{t("practice.page.reading")}</p>}
        {showEmpty && (
          <EmptyState
            className="practice-empty"
            icon={<ClipboardList size={28} aria-hidden="true" />}
            title={t("practice.page.emptyTitle")}
            hint={t("practice.page.emptyHint")}
          />
        )}
        {hasReports && selectedSessionId && <PracticeReportView key={selectedSessionId} sessionId={selectedSessionId} />}
      </section>
      {hasReports && reports && (
        <section className="service-panel practice-history-section" aria-labelledby="practice-history-heading">
          <div className="library-heading">
            <h2 id="practice-history-heading">
              <History size={16} aria-hidden="true" />
              {t("practice.page.historyHeading")}
            </h2>
            <span className="muted">{t("practice.page.historyCount", { n: reports.length })}</span>
          </div>
          <PracticeHistorySection
            reports={reports}
            selectedSessionId={selectedSessionId}
            onSelect={setSelectedSessionId}
          />
        </section>
      )}
    </>
  );
}
