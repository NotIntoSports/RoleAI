// 演示版横幅与首次引导：包裹根组件实现，不改任何业务组件。
// ?capture=1（自动截图用）时隐藏横幅与引导。
import { useMemo, useState, type ReactNode } from "react";

import { isDemoVoiceEnabled, setDemoVoiceEnabled } from "./backend/demo-voice";
import "./demo.css";

const GITHUB_URL = "https://github.com/NotIntoSports/RoleAI";

const ONBOARDING_KEY = "roleai.demo.onboarded";

const GUIDE_STEPS = [
  { title: "选一个角色", detail: "工作台左下角可切换角色：严苛面试官、表达教练、会议助手、直播讲解员。" },
  { title: "开始会话", detail: "点击“开始会话”，演示会自动播放一段脚本对话：字幕逐字出现、AI 流式回答，第 3 轮会演示打断。" },
  { title: "回看记录", detail: "结束会话后到“记录”页回看对话、导出 Markdown；资料与服务页都可以随便点，数据均为虚构。" },
];

function isOnboarded(): boolean {
  try {
    return window.localStorage.getItem(ONBOARDING_KEY) === "1";
  } catch {
    return true;
  }
}

function markOnboarded(): void {
  try {
    window.localStorage.setItem(ONBOARDING_KEY, "1");
  } catch {
    // 存储不可用时就让引导每次出现。
  }
}

export function DemoOverlay({ children }: { children: ReactNode }) {
  const captureMode = useMemo(
    () => new URLSearchParams(window.location.search).get("capture") === "1",
    [],
  );
  const [showGuide, setShowGuide] = useState(() => !captureMode && !isOnboarded());
  const [step, setStep] = useState(0);
  const [voiceEnabled, setVoiceEnabled] = useState(isDemoVoiceEnabled());

  function closeGuide() {
    markOnboarded();
    setShowGuide(false);
  }

  return (
    <div className="demo-root">
      {!captureMode && (
        <div className="demo-banner" role="note" aria-label="演示模式说明">
          <span>
            <strong>在线演示</strong>：所有数据均为虚构，未连接任何 AI 服务。
          </span>
          <a href={`${GITHUB_URL}/releases`} target="_blank" rel="noreferrer">
            下载桌面版体验真实语音对话 →
          </a>
          <label className="demo-banner-toggle">
            <input
              type="checkbox"
              checked={voiceEnabled}
              onChange={(event) => {
                setVoiceEnabled(event.target.checked);
                setDemoVoiceEnabled(event.target.checked);
              }}
            />
            朗读 AI 回答（浏览器语音）
          </label>
        </div>
      )}
      <div className="demo-app-host">{children}</div>
      {showGuide && (
        <div className="demo-guide-backdrop" role="dialog" aria-modal="true" aria-label="在线演示引导">
          <div className="demo-guide">
            <h2>三步玩转在线演示</h2>
            <ol className="demo-guide-steps">
              {GUIDE_STEPS.map((item, index) => (
                <li key={item.title} data-active={index === step}>
                  <span className="demo-guide-step-index" aria-hidden="true">{index + 1}</span>
                  <span>
                    <strong>{item.title}</strong>
                    <br />
                    {item.detail}
                  </span>
                </li>
              ))}
            </ol>
            <div className="demo-guide-actions">
              <button type="button" className="button-ghost" onClick={closeGuide}>
                跳过引导
              </button>
              {step < GUIDE_STEPS.length - 1 ? (
                <button type="button" className="button-primary" onClick={() => setStep(step + 1)}>
                  下一步
                </button>
              ) : (
                <button type="button" className="button-primary" onClick={closeGuide}>
                  开始体验
                </button>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
