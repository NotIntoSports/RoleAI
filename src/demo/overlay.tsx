// 演示版横幅与首次引导：包裹根组件实现，不改任何业务组件。
// ?capture=1（自动截图用）时隐藏横幅与引导。
// 文案按界面语言切换（复用 src/i18n 的语言状态；演示专属文案不进主词典）。
import { useMemo, useState, type ReactNode } from "react";

import { useLanguage } from "../i18n";
import { isDemoVoiceEnabled, setDemoVoiceEnabled } from "./backend/demo-voice";
import "./demo.css";

const GITHUB_URL = "https://github.com/NotIntoSports/RoleAI";

const ONBOARDING_KEY = "roleai.demo.onboarded";

const GUIDE_STEPS = {
  "zh-CN": [
    { title: "选一个角色", detail: "工作台左下角可切换角色：严苛面试官、表达教练、会议助手、直播讲解员。" },
    { title: "开始会话", detail: "点击“开始会话”，演示会自动播放一段脚本对话：字幕逐字出现、AI 流式回答，第 3 轮会演示打断。" },
    { title: "回看记录", detail: "结束会话后到“记录”页回看对话、导出 Markdown；资料与服务页都可以随便点，数据均为虚构。" },
  ],
  en: [
    { title: "Pick a role", detail: "Switch roles from the bottom-left of the workspace: strict interviewer, expression coach, meeting assistant, live presenter." },
    { title: "Start a session", detail: "Click \"Start session\" and the demo plays a scripted conversation: captions appear word by word, the AI streams its reply, and turn 3 demos an interruption." },
    { title: "Review records", detail: "End the session, then review the conversation and export Markdown on the Records page. Materials and Services are safe to explore — all data is fictional." },
  ],
};

const COPY = {
  "zh-CN": {
    bannerAria: "演示模式说明",
    bannerLead: "在线演示",
    bannerText: "：所有数据均为虚构，未连接任何 AI 服务。",
    download: "下载桌面版体验真实语音对话 →",
    voiceToggle: "朗读 AI 回答（浏览器语音）",
    guideAria: "在线演示引导",
    guideHeading: "三步玩转在线演示",
    skip: "跳过引导",
    next: "下一步",
    start: "开始体验",
  },
  en: {
    bannerAria: "Demo mode notice",
    bannerLead: "Online demo",
    bannerText: ": all data is fictional; no AI service is connected.",
    download: "Download the desktop app for real voice conversations →",
    voiceToggle: "Read AI replies aloud (browser speech)",
    guideAria: "Online demo guide",
    guideHeading: "Three steps to try the demo",
    skip: "Skip the guide",
    next: "Next",
    start: "Start exploring",
  },
};

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
  // 语言订阅：useLanguage 已订阅语言状态，语言切换时横幅/引导随之重渲染。
  const language = useLanguage();
  const [showGuide, setShowGuide] = useState(() => !captureMode && !isOnboarded());
  const [step, setStep] = useState(0);
  const [voiceEnabled, setVoiceEnabled] = useState(isDemoVoiceEnabled());

  function closeGuide() {
    markOnboarded();
    setShowGuide(false);
  }

  const copy = COPY[language];
  const steps = GUIDE_STEPS[language];

  return (
    <div className="demo-root">
      {!captureMode && (
        <div className="demo-banner" role="note" aria-label={copy.bannerAria}>
          <span>
            <strong>{copy.bannerLead}</strong>
            {copy.bannerText}
          </span>
          <a href={`${GITHUB_URL}/releases`} target="_blank" rel="noreferrer">
            {copy.download}
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
            {copy.voiceToggle}
          </label>
        </div>
      )}
      <div className="demo-app-host">{children}</div>
      {showGuide && (
        <div className="demo-guide-backdrop" role="dialog" aria-modal="true" aria-label={copy.guideAria}>
          <div className="demo-guide">
            <h2>{copy.guideHeading}</h2>
            <ol className="demo-guide-steps">
              {steps.map((item, index) => (
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
                {copy.skip}
              </button>
              {step < steps.length - 1 ? (
                <button type="button" className="button-primary" onClick={() => setStep(step + 1)}>
                  {copy.next}
                </button>
              ) : (
                <button type="button" className="button-primary" onClick={closeGuide}>
                  {copy.start}
                </button>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
