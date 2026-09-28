import { FormEvent, useCallback, useEffect, useState } from "react";

import * as api from "../../api/commands";
import { EmptyState } from "../../components/empty-state";
import { errorNoticeText as errorText } from "../../components/error-notice";
import type { CommandResult, PublicConfig, RoleProfileConfig } from "../../generated/bindings";

const emptyRole = {
  id: "",
  name: "",
  systemPrompt: "",
  openingMessage: "",
  styleInstructions: "",
};

const optionalId = (value: string) => value.trim() || null;

export function RoleEditor() {
  const [config, setConfig] = useState<PublicConfig | null>(null);
  const [message, setMessage] = useState("正在读取本地配置…");
  const [busy, setBusy] = useState(false);
  const [role, setRole] = useState(emptyRole);
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const result = await api.getConfigPublic();
      if (result.ok) {
        setConfig(result.data);
        setMessage("");
      } else {
        setMessage(errorText(result.error));
      }
    } catch {
      setMessage("IPC_UNAVAILABLE：无法读取本地配置");
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  async function run<T>(action: () => Promise<CommandResult<T>>, success: string) {
    setBusy(true);
    try {
      const result = await action();
      await reload();
      if (!result.ok) {
        setMessage(errorText(result.error));
        return result;
      }
      setMessage(success);
      return result;
    } catch {
      await reload();
      setMessage("IPC_UNAVAILABLE：本地操作失败");
      return null;
    } finally {
      setBusy(false);
    }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    const result = await run(
      () =>
        api.saveRoleProfile({
          id: optionalId(role.id),
          name: role.name.trim(),
          systemPrompt: role.systemPrompt.trim(),
          openingMessage: role.openingMessage.trim(),
          styleInstructions: role.styleInstructions.trim(),
        }),
      "角色已保存",
    );
    if (result?.ok) {
      setRole((current) => ({ ...current, id: result.data.id }));
    }
  }

  const profiles = config?.roleProfiles ?? [];

  return (
    <section className="service-panel role-editor" aria-labelledby="role-editor-heading">
      <h2 className="section-heading" id="role-editor-heading">角色</h2>
      <p className="configuration-description">用提示词、开场白和表达风格定义你的助手。编辑内置模板后保存会创建个人副本。</p>
      {message && (
        <p className="services-message" role="status">
          {message}
        </p>
      )}
      <div className="configuration-columns">
        <form className="service-form configuration-editor" onSubmit={submit}>
          <h3>{role.id && profiles.some((item) => item.id === role.id) ? "编辑角色" : "创建角色"}</h3>
          <label>
            显示名称
            <input
              required
              value={role.name}
              onChange={(event) => setRole({ ...role, name: event.target.value })}
            />
          </label>
          <label>
            系统提示
            <textarea
              maxLength={32 * 1024}
              value={role.systemPrompt}
              onChange={(event) => setRole({ ...role, systemPrompt: event.target.value })}
            />
          </label>
          <label>
            开场白
            <textarea
              maxLength={4 * 1024}
              value={role.openingMessage}
              onChange={(event) => setRole({ ...role, openingMessage: event.target.value })}
            />
          </label>
          <label>
            风格说明
            <textarea
              maxLength={8 * 1024}
              value={role.styleInstructions}
              onChange={(event) => setRole({ ...role, styleInstructions: event.target.value })}
            />
          </label>
          <button className="button-primary" disabled={busy} type="submit">
            保存角色
          </button>
        </form>
        <div className="service-list configuration-list">
          <h3>已有角色 <span className="configuration-count">{profiles.length}</span></h3>
          {profiles.length === 0 && <EmptyState title="还没有角色。" />}
          {profiles.map((item) => (
            <RoleCard
              key={item.id}
              item={item}
              busy={busy}
              pendingDelete={pendingDelete === item.id}
              onEdit={() =>
                setRole({
                  id: item.id,
                  name: item.name,
                  systemPrompt: item.systemPrompt,
                  openingMessage: item.openingMessage,
                  styleInstructions: item.styleInstructions,
                })
              }
              onActivate={() => void run(() => api.activateRoleProfile(item.id), "默认角色已更新")}
              onCopy={() =>
                void run(
                  () =>
                    api.copyRoleProfile({
                      sourceId: item.id,
                      id: null,
                    }),
                  "角色已复制",
                )
              }
              onDelete={() => {
                if (pendingDelete !== item.id) {
                  setPendingDelete(item.id);
                  return;
                }
                void run(() => api.deleteRoleProfile(item.id), "角色已删除").then(() => {
                  setPendingDelete(null);
                });
              }}
            />
          ))}
        </div>
        </div>
      </section>
    );
  }

  function RoleCard(props: {
    item: RoleProfileConfig;
    busy: boolean;
    pendingDelete: boolean;
    onEdit: () => void;
    onActivate: () => void;
    onCopy: () => void;
    onDelete: () => void;
  }) {
    const { item } = props;
    return (
      <article className="service-card">
        <h3>{item.name}</h3>
        {item.active && <span className="status-badge">当前启用</span>}
        {item.configVersion === 0 && <p>需复查后保存才能启用</p>}
        <div className="service-actions">
          <button aria-label={"编辑 " + item.name} disabled={props.busy} onClick={props.onEdit}>
            编辑
          </button>
          <button disabled={props.busy || item.configVersion === 0} onClick={props.onActivate}>
            设为默认
          </button>
          <button disabled={props.busy || item.configVersion === 0} onClick={props.onCopy}>
            复制
          </button>
          <button className="button-danger" disabled={props.busy || item.id.startsWith("preset-")} onClick={props.onDelete}>
            {props.pendingDelete ? "确认删除" : "删除"}
          </button>
        </div>
    </article>
  );
}
