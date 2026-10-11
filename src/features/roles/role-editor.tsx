import { FormEvent, useCallback, useEffect, useState } from "react";

import * as api from "../../api/commands";
import { EmptyState } from "../../components/empty-state";
import { errorNoticeText as errorText } from "../../components/error-notice";
import type { CommandResult, PublicConfig, PublicError, RoleProfileConfig, TavusFace } from "../../generated/bindings";
import { t, useT } from "../../i18n";
import { visibleRoleProfiles } from "./visible-roles";

const emptyRole = {
  id: "",
  name: "",
  systemPrompt: "",
  openingMessage: "",
  styleInstructions: "",
  tavusFaceId: "",
};

const optionalId = (value: string) => value.trim() || null;

export function RoleEditor({ visible = true, onOpenTavus }: { visible?: boolean; onOpenTavus?: () => void }) {
  useT();
  const [config, setConfig] = useState<PublicConfig | null>(null);
  const [message, setMessage] = useState(() => t("roles.reading"));
  const [messageTone, setMessageTone] = useState<"" | "success" | "error">("");
  const [busy, setBusy] = useState(false);
  const [role, setRole] = useState(emptyRole);
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [faces, setFaces] = useState<TavusFace[]>([]);
  const [facesError, setFacesError] = useState<PublicError | null>(null);

  const reload = useCallback(async () => {
    try {
      const result = await api.getConfigPublic();
      if (result.ok) {
        setConfig(result.data);
        setMessage("");
        setMessageTone("");
      } else {
        setMessage(errorText(result.error));
        setMessageTone("error");
      }
    } catch {
      setMessage(t("roles.ipc.statusUnavailable"));
      setMessageTone("error");
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  // 设置页切分类只切显隐，Tavus 密钥可能刚在另一个分类里保存或清除；每次面板变为可见时重拉形象。
  useEffect(() => {
    if (!visible) return;
    let cancelled = false;
    void api.listTavusFaces()
      .then((result) => {
        if (cancelled) return;
        if (result.ok) {
          setFaces(result.data);
          setFacesError(null);
        } else {
          setFaces([]);
          setFacesError(result.error);
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [visible]);

  async function run<T>(action: () => Promise<CommandResult<T>>, success: string) {
    setBusy(true);
    try {
      const result = await action();
      await reload();
      if (!result.ok) {
        setMessage(errorText(result.error));
        setMessageTone("error");
        return result;
      }
      setMessage(success);
      setMessageTone("success");
      return result;
    } catch {
      await reload();
      setMessage(t("roles.ipc.operateFailed"));
      setMessageTone("error");
      return null;
    } finally {
      setBusy(false);
    }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    const submittedId = optionalId(role.id);
    const result = await run(
      () =>
        api.saveRoleProfile({
          id: submittedId,
          name: role.name.trim(),
          systemPrompt: role.systemPrompt.trim(),
          openingMessage: role.openingMessage.trim(),
          styleInstructions: role.styleInstructions.trim(),
          ...(role.tavusFaceId.trim() ? { tavusFaceId: role.tavusFaceId.trim() } : {}),
        }),
      t("roles.saved"),
    );
    if (result?.ok) {
      setRole((current) => ({ ...current, id: result.data.id }));
      if (submittedId && result.data.id !== submittedId) {
        // configVersion 1 才是新建副本；写到已有同名个人角色上时版本会继续增加。
        const createdCopy = result.data.configVersion === 1;
        setMessage(createdCopy ? t("roles.savedAsCopyActive") : t("roles.savedAndCurrent"));
        setMessageTone("success");
      }
    }
  }

  // 列表隐藏与个人角色同名的内置模板（模板仍在配置里，编辑表单仍按 id 识别它们）。
  const profiles = visibleRoleProfiles(config?.roleProfiles ?? []);
  const allProfiles = config?.roleProfiles ?? [];

  return (
    <section className="service-panel role-editor" aria-labelledby="role-editor-heading">
      <h2 className="section-heading" id="role-editor-heading">{t("roles.heading")}</h2>
      <p className="configuration-description">{t("roles.description")}</p>
      <div className="configuration-columns">
        <form className="service-form configuration-editor" onSubmit={submit}>
          <h3>{role.id && allProfiles.some((item) => item.id === role.id) ? t("roles.editTitle") : t("roles.createTitle")}</h3>
          <label>
            {t("roles.nameLabel")}
            <input
              required
              value={role.name}
              onChange={(event) => setRole({ ...role, name: event.target.value })}
            />
          </label>
          <small>{t("roles.nameWakeHint")}</small>
          <label>
            {t("roles.systemPromptLabel")}
            <textarea
              maxLength={32 * 1024}
              value={role.systemPrompt}
              onChange={(event) => setRole({ ...role, systemPrompt: event.target.value })}
            />
          </label>
          <label>
            {t("roles.openingLabel")}
            <textarea
              maxLength={4 * 1024}
              value={role.openingMessage}
              onChange={(event) => setRole({ ...role, openingMessage: event.target.value })}
            />
          </label>
          <label>
            {t("roles.styleLabel")}
            <textarea
              maxLength={8 * 1024}
              value={role.styleInstructions}
              onChange={(event) => setRole({ ...role, styleInstructions: event.target.value })}
            />
          </label>
          <label>
            {t("roles.faceLabel")}
            <select
              value={role.tavusFaceId}
              onChange={(event) => setRole({ ...role, tavusFaceId: event.target.value })}
            >
              <option value="">{t("roles.faceEmpty")}</option>
              {role.tavusFaceId && !faces.some((face) => face.id === role.tavusFaceId) && (
                <option value={role.tavusFaceId}>{role.tavusFaceId}</option>
              )}
              {faces.map((face) => (
                <option key={face.id} value={face.id}>{face.name}</option>
              ))}
            </select>
          </label>
          {facesError ? (
            <>
              <small>{facesError.code === "TAVUS_KEY_MISSING" ? t("roles.faceNeedsKey") : errorText(facesError)}</small>
              {onOpenTavus && (
                <button className="button-ghost" type="button" onClick={onOpenTavus}>
                  {t("roles.faceOpenTavus")}
                </button>
              )}
            </>
          ) : (
            <small>{t("roles.faceHint")}</small>
          )}
          <div className="service-actions">
            <button className="button-primary" disabled={busy} type="submit">
              {t("roles.save")}
            </button>
            {message && (
              <p className="services-message role-save-feedback" role="status" data-tone={messageTone || undefined}>
                {message}
              </p>
            )}
          </div>
        </form>
        <div className="service-list configuration-list">
          <h3>{t("roles.listHeading")} <span className="configuration-count">{profiles.length}</span></h3>
          {profiles.length === 0 && <EmptyState title={t("roles.empty")} />}
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
                  tavusFaceId: item.tavusFaceId ?? "",
                })
              }
              onActivate={() => void run(() => api.activateRoleProfile(item.id), t("roles.defaultUpdated"))}
              onCopy={() =>
                void run(
                  () =>
                    api.copyRoleProfile({
                      sourceId: item.id,
                      id: null,
                    }),
                  t("roles.copied"),
                )
              }
              onDelete={() => {
                if (pendingDelete !== item.id) {
                  setPendingDelete(item.id);
                  return;
                }
                void run(() => api.deleteRoleProfile(item.id), t("roles.deleted")).then(() => {
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
        {item.active && <span className="status-badge">{t("roles.activeBadge")}</span>}
        {item.tavusFaceId && <span className="status-badge">{t("roles.hasFace")}</span>}
        {item.configVersion === 0 && <p>{t("roles.reviewRequired")}</p>}
        <div className="service-actions">
          <button aria-label={t("roles.editAria", { name: item.name })} disabled={props.busy} onClick={props.onEdit}>
            {t("roles.edit")}
          </button>
          <button disabled={props.busy || item.configVersion === 0} onClick={props.onActivate}>
            {t("roles.makeDefault")}
          </button>
          <button disabled={props.busy || item.configVersion === 0} onClick={props.onCopy}>
            {t("roles.copy")}
          </button>
          <button className="button-danger" disabled={props.busy || item.id.startsWith("preset-")} onClick={props.onDelete}>
            {props.pendingDelete ? t("roles.confirmDelete") : t("roles.delete")}
          </button>
        </div>
    </article>
  );
}
