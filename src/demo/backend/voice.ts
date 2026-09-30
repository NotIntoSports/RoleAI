// 语音线路、音色克隆与 OBS 密钥命令（服务页“语音线路/音色克隆”两个分类 + 设置页 OBS 区）。
import type {
  SecretStatus,
  VoiceReferenceSummary,
  VoiceRouteConfig,
  VoiceRouteSaveInput,
} from "../../generated/bindings";

import { demoT } from "./demo-text";
import { getState, updateState } from "./state";
import { demoId, err, latency, ok } from "./util";

export function handleVoiceCommand(cmd: string, payload: Record<string, unknown> = {}): unknown {
  switch (cmd) {
    case "speech_route_save": {
      const input = payload.input as VoiceRouteSaveInput;
      if (!input.name.trim()) return err("SPEECH_ROUTE_INVALID", demoT().voice.routeNameRequired);
      return latency(120, 300).then(() => {
        const existing = input.id
          ? getState().voiceRoutes.find((route) => route.id === input.id) ?? null
          : null;
        const next: VoiceRouteConfig = {
          id: existing?.id ?? demoId("route"),
          name: input.name,
          mode: input.mode,
          asrProviderId: input.asrProviderId,
          asrModelId: input.asrModelId,
          llmProviderId: input.llmProviderId,
          llmModelId: input.llmModelId,
          ttsProviderId: input.ttsProviderId,
          ttsModelId: input.ttsModelId,
          voiceId: input.voiceId,
          e2eProviderId: input.e2eProviderId,
          e2eModelId: input.e2eModelId,
          active: existing?.active ?? false,
          ready: true,
          status: demoT().voice.statusConfigured,
          configVersion: (existing?.configVersion ?? 0) + 1,
        };
        updateState((s) => {
          s.voiceRoutes = existing
            ? s.voiceRoutes.map((route) => (route.id === next.id ? next : route))
            : [...s.voiceRoutes, next];
        });
        return ok(next);
      });
    }
    case "speech_route_test": {
      const routeId = payload.routeId as string;
      const route = getState().voiceRoutes.find((item) => item.id === routeId);
      if (!route) return err("SPEECH_ROUTE_NOT_FOUND", demoT().voice.routeNotFound);
      return latency(200, 600).then(() =>
        ok({
          routeId,
          ready: true,
          checkedProviderIds: [
            route.asrProviderId,
            route.llmProviderId,
            route.ttsProviderId,
            route.e2eProviderId,
          ].filter((id): id is string => Boolean(id)),
        }),
      );
    }
    case "speech_route_activate": {
      const routeId = payload.routeId as string;
      const route = getState().voiceRoutes.find((item) => item.id === routeId);
      if (!route) return err("SPEECH_ROUTE_NOT_FOUND", demoT().voice.routeNotFound);
      if (!route.ready) return err("SPEECH_ROUTE_NOT_READY", demoT().voice.routeNotReady);
      return latency(60, 160).then(() =>
        ok(
          updateState((s) => {
            s.voiceRoutes = s.voiceRoutes.map((item) => ({
              ...item,
              active: item.id === routeId,
            }));
            s.activeVoiceRouteId = routeId;
          }).voiceRoutes.find((item) => item.id === routeId)!,
        ),
      );
    }
    case "speech_route_delete": {
      const routeId = payload.routeId as string;
      return latency(60, 160).then(() => {
        updateState((s) => {
          s.voiceRoutes = s.voiceRoutes.filter((route) => route.id !== routeId);
          if (s.activeVoiceRouteId === routeId) {
            s.activeVoiceRouteId = s.voiceRoutes.find((route) => route.active)?.id ?? null;
          }
        });
        return ok({ ready: true });
      });
    }
    case "voice_reference_list":
      return latency(30, 100).then(() => ok(getState().voiceReferences));
    case "voice_reference_save":
    case "voice_reference_save_audio": {
      const input = payload.input as {
        id: string | null;
        name: string;
        providerId: string | null;
        targetModel: string | null;
        transcript: string | null;
      };
      return latency(150, 400).then(() => {
        const now = new Date().toISOString();
        const existing = input.id
          ? getState().voiceReferences.find((item) => item.id === input.id) ?? null
          : null;
        const next: VoiceReferenceSummary = {
          id: existing?.id ?? demoId("voice"),
          name: input.name,
          providerId: input.providerId,
          targetModel: input.targetModel,
          mimeType: existing?.mimeType ?? "audio/wav",
          byteSize: existing?.byteSize ?? 102400n,
          durationMs: existing?.durationMs ?? 4200n,
          transcript: input.transcript ?? "",
          remoteFileId: existing?.remoteFileId ?? null,
          voiceId: existing?.voiceId ?? null,
          cloneStatus: existing?.cloneStatus ?? "draft",
          cloneError: null,
          createdAt: existing?.createdAt ?? now,
          updatedAt: now,
        };
        updateState((s) => {
          s.voiceReferences = existing
            ? s.voiceReferences.map((item) => (item.id === next.id ? next : item))
            : [...s.voiceReferences, next];
        });
        return ok(next);
      });
    }
    case "voice_reference_update": {
      const input = payload.input as {
        id: string;
        name: string;
        providerId: string | null;
        targetModel: string | null;
        transcript: string | null;
      };
      return latency(80, 200).then(() => {
        const existing = getState().voiceReferences.find((item) => item.id === input.id);
        if (!existing) return err("VOICE_REFERENCE_NOT_FOUND", demoT().voice.voiceNotFound);
        const next: VoiceReferenceSummary = {
          ...existing,
          name: input.name,
          providerId: input.providerId,
          targetModel: input.targetModel,
          transcript: input.transcript ?? "",
          updatedAt: new Date().toISOString(),
        };
        updateState((s) => {
          s.voiceReferences = s.voiceReferences.map((item) => (item.id === next.id ? next : item));
        });
        return ok(next);
      });
    }
    case "voice_reference_clone": {
      const id = payload.id as string;
      const reference = getState().voiceReferences.find((item) => item.id === id);
      if (!reference) return err("VOICE_REFERENCE_NOT_FOUND", demoT().voice.voiceNotFound);
      return latency(400, 900).then(() => {
        const voiceId = `demo-voice-${id.slice(-6)}`;
        const cloned: VoiceReferenceSummary = {
          ...reference,
          voiceId,
          cloneStatus: "ready",
          cloneError: null,
          updatedAt: new Date().toISOString(),
        };
        updateState((s) => {
          s.voiceReferences = s.voiceReferences.map((item) => (item.id === id ? cloned : item));
        });
        return ok({
          referenceId: id,
          voiceId,
          remoteFileId: reference.remoteFileId ?? `demo-remote-${id}`,
        });
      });
    }
    case "voice_reference_delete": {
      const id = payload.id as string;
      return latency(60, 160).then(() => {
        updateState((s) => {
          s.voiceReferences = s.voiceReferences.filter((item) => item.id !== id);
        });
        return ok({ ready: true });
      });
    }
    case "obs_password_status":
      return ok({
        reference: "demo:obs",
        configured: getState().obsPasswordConfigured,
      } satisfies SecretStatus);
    case "obs_password_save": {
      // 演示只记录“已配置”标记，输入的口令立即丢弃。
      return latency(60, 160).then(() => {
        updateState((s) => {
          s.obsPasswordConfigured = true;
        });
        return ok({ reference: "demo:obs", configured: true } satisfies SecretStatus);
      });
    }
    default:
      return undefined;
  }
}
