// 角色命令：role_profile_save / copy / activate / delete。
// 与桌面端同规则：名字 trim 后全局唯一（保存内置模板除外），同名模板在列表里隐藏。
import type { RoleProfileConfig, RoleProfileSaveInput } from "../../generated/bindings";

import { demoT } from "./demo-text";
import { getState, isPresetRoleId, updateState } from "./state";
import { demoId, err, latency, ok } from "./util";

function nextCopyName(roles: RoleProfileConfig[], sourceName: string): string {
  let candidate = `${sourceName}${demoT().roles.copySuffix}`;
  let ordinal = 2;
  while (roles.some((role) => role.name.trim() === candidate)) {
    candidate = `${sourceName}${demoT().roles.copySuffix} ${ordinal}`;
    ordinal += 1;
  }
  return candidate;
}

function toProfile(input: RoleProfileSaveInput, previous: RoleProfileConfig | null): RoleProfileConfig {
  return {
    id: previous?.id ?? demoId("role"),
    name: input.name,
    systemPrompt: input.systemPrompt,
    openingMessage: input.openingMessage,
    styleInstructions: input.styleInstructions,
    tavusFaceId: input.tavusFaceId ?? previous?.tavusFaceId,
    tavusPalId: previous?.tavusPalId,
    tavusSyncedConfigVersion: previous?.tavusSyncedConfigVersion,
    scenario: previous?.scenario,
    active: previous?.active ?? false,
    configVersion: (previous?.configVersion ?? 0) + 1,
  };
}

export function handleRoleCommand(cmd: string, payload: Record<string, unknown> = {}): unknown {
  switch (cmd) {
    case "role_profile_save": {
      const input = payload.input as RoleProfileSaveInput;
      if (!input.name.trim()) {
        return err("ROLE_PROFILE_INVALID", demoT().roles.nameRequired);
      }
      const savingPreset = input.id ? isPresetRoleId(input.id) : false;
      const existing = input.id
        ? getState().roleProfiles.find((role) => role.id === input.id) ?? null
        : null;
      if (!savingPreset) {
        const conflict = getState().roleProfiles.some(
          (role) => role.id !== (input.id ?? "") && role.name.trim() === input.name.trim(),
        );
        if (conflict) {
          return err("ROLE_PROFILE_FIELDS_INVALID", demoT().roles.nameTaken(input.name.trim()));
        }
      }
      return latency(80, 200).then(() => {
        const next = toProfile(input, existing);
        updateState((s) => {
          s.roleProfiles = existing
            ? s.roleProfiles.map((role) => (role.id === next.id ? next : role))
            : [...s.roleProfiles, next];
        });
        return ok(next);
      });
    }
    case "role_profile_copy": {
      const input = payload.input as { sourceId: string; id: string | null };
      const s = getState();
      const source = s.roleProfiles.find((role) => role.id === input.sourceId);
      if (!source) return err("ROLE_PROFILE_NOT_FOUND", demoT().roles.copySourceMissing);
      return latency(80, 200).then(() => {
        const copy: RoleProfileConfig = {
          ...source,
          id: input.id ?? demoId("role"),
          name: nextCopyName(s.roleProfiles, source.name.trim()),
          tavusPalId: undefined,
          tavusSyncedConfigVersion: undefined,
          scenario: undefined,
          active: false,
          configVersion: 1,
        };
        updateState((draft) => {
          draft.roleProfiles = [...draft.roleProfiles, copy];
        });
        return ok(copy);
      });
    }
    case "role_profile_activate": {
      const roleId = payload.roleId as string;
      const s = getState();
      const role = s.roleProfiles.find((item) => item.id === roleId);
      if (!role) return err("ROLE_PROFILE_NOT_FOUND", demoT().roles.notFound);
      return latency(60, 160).then(() =>
        ok(
          updateState((draft) => {
            draft.roleProfiles = draft.roleProfiles.map((item) => ({
              ...item,
              active: item.id === roleId,
            }));
            draft.activeRoleProfileId = roleId;
            return { ...role, active: true };
          }).roleProfiles.find((item) => item.id === roleId)!,
        ),
      );
    }
    case "role_profile_delete": {
      const roleId = payload.roleId as string;
      return latency(60, 160).then(() => {
        const s = getState();
        if (!s.roleProfiles.some((item) => item.id === roleId)) {
          return err("ROLE_PROFILE_NOT_FOUND", demoT().roles.notFound);
        }
        updateState((draft) => {
          draft.roleProfiles = draft.roleProfiles.filter((item) => item.id !== roleId);
          if (draft.activeRoleProfileId === roleId) {
            draft.activeRoleProfileId = draft.roleProfiles[0]?.id ?? null;
          }
        });
        return ok({ ready: true });
      });
    }
    default:
      return undefined;
  }
}
