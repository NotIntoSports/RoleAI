import { useCallback, useEffect, useState } from "react";
import { ClipboardList } from "lucide-react";

import * as api from "../../api/commands";
import { EmptyState } from "../../components/empty-state";
import { PracticeReportView } from "../../features/practice/practice-report";
import { PracticeWizard } from "../../features/practice/practice-wizard";
import { PageShell } from "../page-shell";

export function PracticePage() {
  // 报告区默认展示最近一次已生成的训练报告；训练历史（E09）提供逐条选择。
  const [latestSessionId, setLatestSessionId] = useState<string | null>(null);
  const [phase, setPhase] = useState<"loading" | "ready" | "empty">("loading");

  const loadLatest = useCallback(async () => {
    setPhase("loading");
    try {
      const result = await api.listPracticeReports();
      if (result.ok && result.data.length > 0) {
        setLatestSessionId(result.data[0].sessionId);
        setPhase("ready");
      } else if (result.ok) {
        setLatestSessionId(null);
        setPhase("empty");
      } else {
        setLatestSessionId(null);
        setPhase("empty");
      }
    } catch {
      setLatestSessionId(null);
      setPhase("empty");
    }
  }, []);

  useEffect(() => {
    void loadLatest();
  }, [loadLatest]);

  return (
    <>
      <PageShell id="practice" />
      <PracticeWizard />
      <section className="service-panel practice-report-section" aria-labelledby="practice-report-heading">
        <div className="library-heading">
          <h2 id="practice-report-heading">
            <ClipboardList size={16} aria-hidden="true" />
            训练报告
          </h2>
          {phase === "ready" && <span className="muted">最近一次训练</span>}
        </div>
        {phase === "loading" && <p className="muted" role="status">正在读取训练报告…</p>}
        {phase === "empty" && (
          <EmptyState
            className="practice-empty"
            icon={<ClipboardList size={28} aria-hidden="true" />}
            title="还没有训练报告。"
            hint="完成一次模拟面试训练并生成报告后，这里会展示总分、维度评分与逐题点评。"
          />
        )}
        {phase === "ready" && latestSessionId && <PracticeReportView sessionId={latestSessionId} />}
      </section>
    </>
  );
}
