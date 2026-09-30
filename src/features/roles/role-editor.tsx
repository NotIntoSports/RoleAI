import { FormEvent, useCallback, useEffect, useState } from "react";

import * as api from "../../api/commands";
import { EmptyState } from "../../components/empty-state";
import { errorNoticeText as errorText } from "../../components/error-notice";
import type { CommandResult, PublicConfig, RoleProfileConfig } from "../../generated/bindings";
import { t, useT } from "../../i18n";

const emptyRole = {
  id: "",
  name: "",
  systemPrompt: "",
  openingMessage: "",
  styleInstructions: "",
};

const optionalId = (value: string) => value.trim() || null;

export function RoleEditor() {
  useT();
  const [config, setConfig] = useState<PublicConfig | null>(null);
  const [message, setMessage] = useState(() => t("roles.reading"));
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
      setMessage(t("roles.ipc.statusUnavailable"));
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
      setMessage(t("roles.ipc.operateFailed"));
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
      t("roles.saved"),
    );
    if (result?.ok) {
      setRole((current) => ({ ...current, id: result.data.id }));
    }
  }

  const profiles = config?.roleProfiles ?? [];

  return (
    <section className="service-panel role-editor" aria-labelledby="role-editor-heading">
      <h2 className="section-heading" id="role-editor-heading">{t("roles.heading")}</h2>
      <p className="configuration-description">{t("roles.description")}</p>
      {message && (
        <p className="services-message" role="status">
          {message}
        </p>
      )}
      <div className="configuration-columns">
        <form className="service-form configuration-editor" onSubmit={submit}>
          <h3>{role.id && profiles.some((item) => item.id === role.id) ? t("roles.editTitle") : t("roles.createTitle")}</h3>
          <label>
            {t("roles.nameLabel")}
            <input
              required
              value={role.name}
              onChange={(event) => setRole({ ...role, name: event.target.value })}
            />
          </label>
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
          <button className="button-primary" disabled={busy} type="submit">
            {t("roles.save")}
          </button>
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
