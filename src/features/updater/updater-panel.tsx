import { useState } from "react";
import { RefreshCw } from "lucide-react";
import { check } from "@tauri-apps/plugin-updater";

/**
 * 设置页「检查更新」面板（G07）：只做手动检查，不做静默自动更新。
 * 签名公钥未配置时（当前发布态），插件 check() 会报错，界面如实显示
 * 「更新源未配置或不可达」，不影响应用其它功能。
 */

/** 面板需要的最小更新对象结构；官方 Update 类满足该接口（便于测试注入替身）。 */
export interface InstallableUpdate {
  version: string;
  body?: string;
  downloadAndInstall: () => Promise<void>;
}

export type UpdateCheckResult =
  | { status: "upToDate" }
  | { status: "available"; update: InstallableUpdate };

type CheckForUpdate = () => Promise<UpdateCheckResult>;

type UpdaterState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "upToDate" }
  | { kind: "available"; update: InstallableUpdate }
  | { kind: "installing" }
  | { kind: "error"; message: string };

/** 默认实现：包装官方 updater 插件。测试通过 props 注入替身。 */
const defaultCheckForUpdate: CheckForUpdate = async () => {
  const update = await check();
  return update ? { status: "available", update } : { status: "upToDate" };
};

export function UpdaterPanel({ checkForUpdate = defaultCheckForUpdate }: { checkForUpdate?: CheckForUpdate }) {
  const [state, setState] = useState<UpdaterState>({ kind: "idle" });

  const runCheck = () => {
    setState({ kind: "checking" });
    void checkForUpdate()
      .then((result) => {
        if (result.status === "upToDate") setState({ kind: "upToDate" });
        else setState({ kind: "available", update: result.update });
      })
      .catch((error: unknown) => {
        setState({
          kind: "error",
          message: error instanceof Error ? error.message : String(error),
        });
      });
  };

  const install = (update: InstallableUpdate) => {
    setState({ kind: "installing" });
    void update
      .downloadAndInstall()
      .then(() => {
        // Windows 上插件会在安装阶段自动退出应用并完成安装；异常时回到可重试状态。
        setState({ kind: "idle" });
      })
      .catch((error: unknown) => {
        setState({
          kind: "error",
          message: error instanceof Error ? error.message : String(error),
        });
      });
  };

  const busy = state.kind === "checking" || state.kind === "installing";
  return (
    <section className="service-panel updater-panel" aria-labelledby="updater-heading">
      <div className="section-heading">
        <h2 id="updater-heading">应用更新</h2>
        <p className="muted">手动检查新版本；不会在后台自动下载或安装。</p>
      </div>
      <div className="updater-actions">
        <button type="button" className="button-secondary" disabled={busy} onClick={runCheck}>
          <RefreshCw size={14} aria-hidden="true" />{state.kind === "checking" ? "正在检查…" : "检查更新"}
        </button>
        {state.kind === "upToDate" && <span className="muted" role="status">当前已是最新版本。</span>}
        {state.kind === "available" && (
          <span className="muted" role="status">
            发现新版本 {state.update.version}。
            <button type="button" className="button-primary" disabled={busy} onClick={() => install(state.update)}>
              下载并安装
            </button>
          </span>
        )}
        {state.kind === "installing" && <span className="muted" role="status">正在下载并安装更新，完成后应用会自动重启…</span>}
        {state.kind === "error" && (
          <span className="muted" role="alert">
            更新源未配置或不可达：{state.message}。待发布签名公钥配置后即可检查更新。
          </span>
        )}
      </div>
    </section>
  );
}
