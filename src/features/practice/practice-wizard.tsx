import { FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import { Link } from "wouter";
import { ClipboardList, FileText, FolderOpen, Sparkles, Trash2 } from "lucide-react";

import * as api from "../../api/commands";
import { EmptyState } from "../../components/empty-state";
import { errorNoticeText as errorText } from "../../components/error-notice";
import type { MaterialSummary, PracticePlan, PracticeQuestion } from "../../generated/bindings";
import "../../styles/practice.css";

const WIZARD_STEPS = ["岗位与资料", "面试官风格", "题量与时长", "预览并编辑题单"] as const;

const INTERVIEWER_STYLES = [
  { value: "面试官", hint: "常规面试：围绕岗位要求与项目经历提问。" },
  { value: "HR", hint: "行为面试：考察动机、协作与稳定性。" },
  { value: "严苛面试官", hint: "高压追问：直指薄弱点，考察临场应变。" },
] as const;

const DIFFICULTIES = ["基础", "标准", "进阶"] as const;

const MIN_QUESTIONS = 1;
const MAX_QUESTIONS = 12;
/** 估算口径与后端提示词一致：每题回答约 3～5 分钟。 */
const MINUTES_PER_QUESTION: [number, number] = [3, 5];

interface PracticeWizardProps {
  /** 测试注入：跳过真实路由跳转。 */
  materialsLink?: string;
}

export function PracticeWizard({ materialsLink = "/materials" }: PracticeWizardProps = {}) {
  const [step, setStep] = useState(0);
  const [position, setPosition] = useState("");
  const [jdMaterialId, setJdMaterialId] = useState("");
  const [resumeMaterialId, setResumeMaterialId] = useState("");
  const [interviewerStyle, setInterviewerStyle] = useState<string>(INTERVIEWER_STYLES[0].value);
  const [questionCount, setQuestionCount] = useState(5);
  const [difficulty, setDifficulty] = useState<string>(DIFFICULTIES[1]);
  const [materials, setMaterials] = useState<MaterialSummary[]>([]);
  const [materialsState, setMaterialsState] = useState<"loading" | "ready" | "error">("loading");
  const [plan, setPlan] = useState<PracticePlan | null>(null);
  const [message, setMessage] = useState("");
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);

  const reloadMaterials = useCallback(async () => {
    setMaterialsState("loading");
    try {
      const result = await api.listMaterials();
      if (result.ok) {
        setMaterials(result.data);
        setMaterialsState("ready");
      } else {
        setMessage(errorText(result.error));
        setMaterialsState("error");
      }
    } catch {
      setMessage("IPC_UNAVAILABLE：无法读取本地资料");
      setMaterialsState("error");
    }
  }, []);

  useEffect(() => {
    void reloadMaterials();
  }, [reloadMaterials]);

  const estimatedMinutes = useMemo(() => {
    const [minPer, maxPer] = MINUTES_PER_QUESTION;
    return `${questionCount * minPer}~${questionCount * maxPer}`;
  }, [questionCount]);

  const positionValid = position.trim().length > 0 && [...position.trim()].length <= 80;
  const planEditable = !!plan && plan.questions.length >= MIN_QUESTIONS && plan.questions.every((q) => q.prompt.trim().length > 0);

  function updateQuestionPrompt(index: number, prompt: string) {
    setPlan((current) => {
      if (!current) return current;
      const questions = current.questions.map((question, i) => (i === index ? { ...question, prompt } : question));
      return { ...current, questions };
    });
    setSaved(false);
  }

  function removeQuestion(index: number) {
    setPlan((current) => {
      if (!current) return current;
      return { ...current, questions: current.questions.filter((_, i) => i !== index) };
    });
    setSaved(false);
  }

  async function handleGenerate() {
    setBusy(true);
    setMessage("");
    try {
      const result = await api.generatePracticePlan({
        position: position.trim(),
        jdMaterialId: jdMaterialId || undefined,
        resumeMaterialId: resumeMaterialId || undefined,
        interviewerStyle,
        questionCount,
        difficulty,
      });
      if (result.ok) {
        setPlan(result.data);
        setSaved(false);
      } else {
        setMessage(errorText(result.error));
      }
    } catch {
      setMessage("IPC_UNAVAILABLE：题单生成失败");
    } finally {
      setBusy(false);
    }
  }

  async function handleSave(event: FormEvent) {
    event.preventDefault();
    if (!plan) return;
    if (!planEditable) {
      setMessage("题单至少保留 1 道题目，且每道题面不能为空。");
      return;
    }
    setBusy(true);
    setMessage("");
    try {
      const result = await api.savePracticePlan(plan);
      if (result.ok) {
        setPlan(result.data);
        setSaved(true);
        setMessage("");
      } else {
        setMessage(errorText(result.error));
      }
    } catch {
      setMessage("IPC_UNAVAILABLE：题单保存失败");
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="service-panel practice-wizard" aria-labelledby="practice-wizard-heading">
      <div className="library-heading">
        <h2 id="practice-wizard-heading">训练准备</h2>
        <span className="muted">第 {step + 1} 步，共 {WIZARD_STEPS.length} 步</span>
      </div>
      <ol className="practice-steps" aria-label="向导步骤">
        {WIZARD_STEPS.map((label, index) => (
          <li
            key={label}
            data-current={index === step ? "true" : undefined}
            data-done={index < step ? "true" : undefined}
            aria-current={index === step ? "step" : undefined}
          >
            {label}
          </li>
        ))}
      </ol>
      {message && (
        <p className="services-message" role="status">
          {message}
        </p>
      )}

      {step === 0 && (
        <div className="practice-step-panel" aria-label="岗位与资料">
          <form className="service-form" onSubmit={(event) => { event.preventDefault(); if (positionValid) setStep(1); }}>
            <label>
              岗位方向
              <input
                value={position}
                placeholder="例如：后端开发工程师"
                maxLength={80}
                onChange={(event) => setPosition(event.target.value)}
              />
            </label>
            {materialsState === "loading" && <p className="muted" role="status">正在读取本地资料…</p>}
            {materialsState === "error" && (
              <p className="muted">资料列表读取失败，可稍后在资料页重试；不选资料也能按岗位方向出题。</p>
            )}
            {materialsState === "ready" && materials.length === 0 && (
              <EmptyState
                className="practice-empty"
                icon={<FolderOpen size={28} aria-hidden="true" />}
                title="还没有资料。"
                hint="导入岗位 JD 和个人简历，题单会更贴合你的目标。"
              />
            )}
            {materials.length > 0 && (
              <>
                <label>
                  岗位 JD（可选）
                  <select value={jdMaterialId} onChange={(event) => setJdMaterialId(event.target.value)}>
                    <option value="">不使用</option>
                    {materials.map((item) => (
                      <option key={item.id} value={item.id}>{item.fileName}</option>
                    ))}
                  </select>
                </label>
                <label>
                  个人简历（可选）
                  <select value={resumeMaterialId} onChange={(event) => setResumeMaterialId(event.target.value)}>
                    <option value="">不使用</option>
                    {materials.map((item) => (
                      <option key={item.id} value={item.id}>{item.fileName}</option>
                    ))}
                  </select>
                </label>
                <p className="muted practice-hint">
                  没有合适的资料？<Link href={materialsLink}>去资料页导入 JD 或简历</Link>。
                </p>
              </>
            )}
            {materials.length === 0 && materialsState === "ready" && (
              <p className="muted practice-hint">
                <Link href={materialsLink}>去资料页导入 JD 或简历</Link>（可稍后再补，先按岗位方向出题也可以）。
              </p>
            )}
            <div className="service-actions practice-nav">
              <span />
              <button className="button-primary" type="submit" disabled={!positionValid}>
                下一步
              </button>
            </div>
          </form>
        </div>
      )}

      {step === 1 && (
        <div className="practice-step-panel" aria-label="面试官风格">
          <fieldset className="practice-style-group">
            <legend>选择一位 AI 面试官</legend>
            {INTERVIEWER_STYLES.map((style) => (
              <label key={style.value} className="practice-style-option">
                <input
                  type="radio"
                  name="practice-interviewer-style"
                  value={style.value}
                  checked={interviewerStyle === style.value}
                  onChange={() => setInterviewerStyle(style.value)}
                />
                <span className="practice-style-name">{style.value}</span>
                <span className="muted">{style.hint}</span>
              </label>
            ))}
          </fieldset>
          <div className="service-actions practice-nav">
            <button className="button-ghost" type="button" onClick={() => setStep(0)}>
              上一步
            </button>
            <button className="button-primary" type="button" onClick={() => setStep(2)}>
              下一步
            </button>
          </div>
        </div>
      )}

      {step === 2 && (
        <div className="practice-step-panel" aria-label="题量与时长">
          <form className="service-form" onSubmit={(event) => { event.preventDefault(); setStep(3); }}>
            <label>
              题量（1~{MAX_QUESTIONS} 题）
              <input
                type="number"
                min={MIN_QUESTIONS}
                max={MAX_QUESTIONS}
                value={questionCount}
                onChange={(event) => {
                  const next = Number(event.target.value);
                  if (Number.isFinite(next)) setQuestionCount(Math.min(MAX_QUESTIONS, Math.max(MIN_QUESTIONS, Math.round(next))));
                }}
              />
            </label>
            <label>
              难度
              <select value={difficulty} onChange={(event) => setDifficulty(event.target.value)}>
                {DIFFICULTIES.map((item) => (
                  <option key={item} value={item}>{item}</option>
                ))}
              </select>
            </label>
            <p className="muted practice-hint">
              按每题 3~5 分钟估算，本轮训练预计约 {estimatedMinutes} 分钟（以实际对练节奏为准）。
            </p>
            <div className="service-actions practice-nav">
              <button className="button-ghost" type="button" onClick={() => setStep(1)}>
                上一步
              </button>
              <button className="button-primary" type="submit">
                下一步
              </button>
            </div>
          </form>
        </div>
      )}

      {step === 3 && (
        <div className="practice-step-panel" aria-label="预览并编辑题单">
          {!plan && (
            <div className="practice-generate">
              <p>
                岗位：<strong>{position.trim()}</strong>
                <span className="muted"> · 风格：{interviewerStyle} · 难度：{difficulty} · {questionCount} 题</span>
              </p>
              <button className="button-primary" type="button" disabled={busy} onClick={() => void handleGenerate()}>
                <Sparkles size={16} aria-hidden="true" />
                {busy ? "正在生成题单…" : "生成题单"}
              </button>
              <p className="muted">生成调用你配置的模型服务；生成后可以逐题编辑再保存。</p>
            </div>
          )}
          {plan && (
            <form className="practice-plan-editor" onSubmit={handleSave}>
              <div className="library-heading">
                <h3>
                  <ClipboardList size={16} aria-hidden="true" />
                  {plan.title}
                </h3>
                <span className="muted">{plan.questions.length} 道题</span>
              </div>
              <ol className="practice-question-list">
                {plan.questions.map((question, index) => (
                  <QuestionRow
                    key={index}
                    index={index}
                    question={question}
                    busy={busy}
                    onPromptChange={(prompt) => updateQuestionPrompt(index, prompt)}
                    onRemove={() => removeQuestion(index)}
                  />
                ))}
              </ol>
              <div className="service-actions practice-nav">
                <button className="button-ghost" type="button" disabled={busy} onClick={() => void handleGenerate()}>
                  重新生成
                </button>
                <button className="button-primary" type="submit" disabled={busy || !planEditable}>
                  {saved ? "已保存" : "保存题单"}
                </button>
              </div>
              {saved && <p className="muted" role="status">题单已保存，可到训练时使用。</p>}
            </form>
          )}
        </div>
      )}
    </section>
  );
}

interface QuestionRowProps {
  index: number;
  question: PracticeQuestion;
  busy: boolean;
  onPromptChange: (prompt: string) => void;
  onRemove: () => void;
}

function QuestionRow({ index, question, busy, onPromptChange, onRemove }: QuestionRowProps) {
  return (
    <li className="practice-question-row">
      <div className="practice-question-head">
        <FileText size={15} aria-hidden="true" />
        <span className="muted">第 {index + 1} 题</span>
        {question.focus && <span className="status-badge">{question.focus}</span>}
        <button className="button-ghost practice-question-remove" type="button" disabled={busy} aria-label={`删除第 ${index + 1} 题`} onClick={onRemove}>
          <Trash2 size={14} aria-hidden="true" />
          删除
        </button>
      </div>
      <label className="practice-question-prompt">
        <span className="muted">第 {index + 1} 题题面</span>
        <textarea
          value={question.prompt}
          rows={2}
          maxLength={500}
          onChange={(event) => onPromptChange(event.target.value)}
        />
      </label>
      {question.expectedPoints.length > 0 && (
        <p className="muted practice-question-points">期望要点：{question.expectedPoints.join("、")}</p>
      )}
      {question.followups.length > 0 && (
        <p className="muted practice-question-points">可能追问：{question.followups.join("、")}</p>
      )}
    </li>
  );
}
