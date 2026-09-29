// 模型供应商与 Embedding 命令。密钥只保留“已配置”标记，永远不落任何密钥材料。
import type {
  EmbeddingConfig,
  EmbeddingConfigSaveInput,
  ProviderConfig,
  ProviderSaveInput,
} from "../../generated/bindings";

import { getState, updateState } from "./state";
import { demoId, err, latency, ok } from "./util";

const DEMO_MODELS = [
  "qwen3.8-omni-flash-realtime（演示）",
  "qwen-plus（演示）",
  "qwen-max（演示）",
  "text-embedding-v4（演示）",
];

export function handleProviderCommand(cmd: string, payload: Record<string, unknown> = {}): unknown {
  switch (cmd) {
    case "model_provider_save": {
      const input = payload.input as ProviderSaveInput;
      if (!input.baseUrl.trim()) return err("PROVIDER_ENDPOINT_INVALID", "接入地址不能为空。");
      return latency(120, 300).then(() => {
        const existing = input.id
          ? getState().providers.find((provider) => provider.id === input.id) ?? null
          : null;
        const next: ProviderConfig = {
          webCapability: input.webCapability,
          id: existing?.id ?? demoId("provider"),
          name: input.name,
          baseUrl: input.baseUrl,
          credential: {
            reference: `demo:${existing?.id ?? "provider"}`,
            configured: Boolean(input.apiKey) || Boolean(existing?.credential?.configured),
          },
        };
        updateState((s) => {
          s.providers = existing
            ? s.providers.map((provider) => (provider.id === next.id ? next : provider))
            : [...s.providers, next];
          if (!s.activeProviderId) s.activeProviderId = next.id;
          if (input.apiKey) s.providerSecrets[next.id] = true;
        });
        return ok(next);
      });
    }
    case "model_provider_test": {
      const providerId = payload.providerId as string;
      const provider = getState().providers.find((item) => item.id === providerId);
      if (!provider) return err("PROVIDER_NOT_FOUND", "找不到该供应商。");
      // 卡片要求：测试连接返回成功 + 200～600ms 模拟延迟。
      return latency(200, 600).then(() =>
        ok({
          providerId,
          reachable: true,
          modelCount: DEMO_MODELS.length,
          webStatus: provider.webCapability && provider.webCapability !== "none" ? "available" : "disabled",
          webSourceCount: 3,
        }),
      );
    }
    case "model_provider_discover":
      return latency(200, 600).then(() =>
        ok({
          providerId: payload.providerId as string,
          models: DEMO_MODELS.map((id) => ({ id })),
        }),
      );
    case "model_provider_activate": {
      const providerId = payload.providerId as string;
      const provider = getState().providers.find((item) => item.id === providerId);
      if (!provider) return err("PROVIDER_NOT_FOUND", "找不到该供应商。");
      return latency(60, 160).then(() => {
        updateState((s) => {
          s.activeProviderId = providerId;
        });
        return ok(provider);
      });
    }
    case "model_provider_dependencies": {
      const providerId = payload.providerId as string;
      const s = getState();
      const references = [
        ...s.voiceRoutes
          .filter(
            (route) => route.asrProviderId === providerId || route.llmProviderId === providerId
              || route.ttsProviderId === providerId || route.e2eProviderId === providerId,
          )
          .map((route) => ({ kind: "voiceRoute", id: route.id, name: route.name })),
        ...s.embeddingConfigs
          .filter((embedding) => embedding.providerId === providerId)
          .map((embedding) => ({ kind: "embedding", id: embedding.id, name: embedding.modelId })),
      ];
      return latency(30, 100).then(() => ok(references));
    }
    case "model_provider_delete": {
      const providerId = payload.providerId as string;
      return latency(80, 200).then(() => {
        const s = getState();
        const inUse =
          s.voiceRoutes.some(
            (route) => route.asrProviderId === providerId || route.llmProviderId === providerId
              || route.ttsProviderId === providerId || route.e2eProviderId === providerId,
          ) || s.embeddingConfigs.some((embedding) => embedding.providerId === providerId);
        if (inUse) {
          return err("PROVIDER_IN_USE", "供应商仍被语音线路或 Embedding 配置引用。");
        }
        updateState((draft) => {
          draft.providers = draft.providers.filter((provider) => provider.id !== providerId);
          delete draft.providerSecrets[providerId];
          if (draft.activeProviderId === providerId) {
            draft.activeProviderId = draft.providers[0]?.id ?? null;
          }
        });
        return ok({ ready: true });
      });
    }
    case "embedding_config_save": {
      const input = payload.input as EmbeddingConfigSaveInput;
      return latency(120, 300).then(() => {
        const existing = input.id
          ? getState().embeddingConfigs.find((item) => item.id === input.id) ?? null
          : null;
        const next: EmbeddingConfig = {
          id: existing?.id ?? demoId("embedding"),
          providerId: input.providerId,
          baseUrl: input.baseUrl,
          credential: {
            reference: `demo:${existing?.id ?? "embedding"}`,
            configured: Boolean(input.apiKey) || Boolean(existing?.credential?.configured),
          },
          modelId: input.modelId,
          dimensions: input.dimensions,
          distance: "cosine",
          normalized: input.normalized,
          active: existing?.active ?? false,
          ready: true,
          status: "已配置（演示）",
          configVersion: (existing?.configVersion ?? 0) + 1,
        };
        updateState((s) => {
          s.embeddingConfigs = existing
            ? s.embeddingConfigs.map((item) => (item.id === next.id ? next : item))
            : [...s.embeddingConfigs, next];
        });
        return ok(next);
      });
    }
    case "embedding_config_test":
      return latency(200, 600).then(() =>
        ok({ id: payload.embeddingId as string, ready: true, dimensions: 1024 }),
      );
    case "embedding_config_activate": {
      const embeddingId = payload.embeddingId as string;
      const embedding = getState().embeddingConfigs.find((item) => item.id === embeddingId);
      if (!embedding) return err("EMBEDDING_NOT_FOUND", "找不到该 Embedding 配置。");
      return latency(60, 160).then(() =>
        ok(
          updateState((s) => {
            s.embeddingConfigs = s.embeddingConfigs.map((item) => ({
              ...item,
              active: item.id === embeddingId,
            }));
            s.activeEmbeddingConfigId = embeddingId;
          }).embeddingConfigs.find((item) => item.id === embeddingId)!,
        ),
      );
    }
    case "embedding_config_delete": {
      const embeddingId = payload.embeddingId as string;
      return latency(60, 160).then(() => {
        updateState((s) => {
          s.embeddingConfigs = s.embeddingConfigs.filter((item) => item.id !== embeddingId);
          if (s.activeEmbeddingConfigId === embeddingId) {
            s.activeEmbeddingConfigId = s.embeddingConfigs.find((item) => item.active)?.id ?? null;
          }
        });
        return ok({ ready: true });
      });
    }
    default:
      return undefined;
  }
}
