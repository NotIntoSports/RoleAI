// 公共配置视图与启动相关命令（config_get_public / startup / 诊断 / 迁移）。
import type {
  DiagnosticsLatencySummary,
  PublicConfig,
  RouteLatencySummary,
  StartupState,
} from "../../generated/bindings";

import { getState, resetDemoState } from "./state";
import { latency, ok } from "./util";

export function publicConfig(): PublicConfig {
  const s = getState();
  return {
    configVersion: s.configVersion,
    application: { locale: null },
    models: { providers: s.providers, activeProviderId: s.activeProviderId },
    speech: { voiceRoutes: s.voiceRoutes, activeVoiceRouteId: s.activeVoiceRouteId },
    knowledge: {
      embeddingConfigs: s.embeddingConfigs,
      activeEmbeddingConfigId: s.activeEmbeddingConfigId,
    },
    storage: { exportDirectory: null },
    roleProfiles: s.roleProfiles,
    activeRoleProfileId: s.activeRoleProfileId,
    diagnostics: { logRetentionDays: 14 },
  };
}

export function handleConfigCommand(cmd: string, payload: Record<string, unknown> = {}): unknown {
  switch (cmd) {
    case "config_get_startup_state":
      return ok({ kind: "ready" } satisfies StartupState);
    case "config_get_public":
      return ok(publicConfig());
    case "config_restore_last_good":
    case "config_restore_defaults":
      // 演示里“恢复默认”就是重置演示数据（重新播种并清掉本地持久化）。
      return latency(120, 300).then(() => {
        resetDemoState();
        return ok({ kind: "ready" } satisfies StartupState);
      });
    case "open_app_directory":
      return ok({ ready: true });
    case "legacy_migration_status":
      return ok({ applied: true, reenterSecrets: false, omitted: [] });
    case "diagnostics_latency_summary": {
      const limit = typeof payload.limit === "number" && payload.limit > 0 ? payload.limit : 20;
      const s = getState();
      const routes: RouteLatencySummary[] = s.voiceRoutes
        .filter((route) => route.ready)
        .slice(0, limit)
        .map((route, index) => ({
          routeId: route.id,
          routeLabel: route.name,
          mode: index === 0 ? "realtime" : "cascade",
          samples: 24 - index * 6,
          p50Ms: 640 - index * 40,
          p95Ms: 1180 - index * 60,
          // 演示数值为虚构（与演示横幅声明一致）：按线路模式给分阶段 p50/p95。
          stages:
            index === 0
              ? [
                  { stage: "responseCreatedMs", samples: 24, p50Ms: 120, p95Ms: 260 },
                  { stage: "firstAudioMs", samples: 24, p50Ms: 380, p95Ms: 820 },
                ]
              : [
                  { stage: "asrDoneMs", samples: 18, p50Ms: 210, p95Ms: 430 },
                  { stage: "llmFirstTokenMs", samples: 18, p50Ms: 360, p95Ms: 700 },
                  { stage: "ttsDoneMs", samples: 18, p50Ms: 600, p95Ms: 1150 },
                ],
          ingressDroppedTotal: 0,
        }));
      const summary: DiagnosticsLatencySummary = {
        sessionsScanned: 3,
        routes,
        recentTurns: routes.flatMap((route, routeIndex) =>
          Array.from({ length: routeIndex === 0 ? 8 : 5 }, (_, turnIndex) => ({
            routeId: route.routeId,
            mode: route.mode,
            totalMs: route.p50Ms === null ? null : route.p50Ms + ((turnIndex * 47) % 240) - 120,
            createdAt: new Date(
              Date.UTC(2026, 8, 30, 10, routeIndex * 10 + turnIndex, 0),
            ).toISOString(),
          })),
        ),
      };
      return ok(summary);
    }
    case "diagnostics_export":
      return ok({ exported: false });
    case "foundation_get_status":
      return latency(40, 120).then(() => ok({ ready: true }));
    default:
      return undefined;
  }
}
