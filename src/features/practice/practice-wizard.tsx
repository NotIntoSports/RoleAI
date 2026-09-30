import { FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import { Link } from "wouter";
import { useHashLocation } from "wouter/use-hash-location";
import { ClipboardList, FileText, FolderOpen, Play, Sparkles, Trash2 } from "lucide-react";

import * as api from "../../api/commands";
import { EmptyState } from "../../components/empty-state";
import { errorNoticeText as errorText } from "../../components/error-notice";
import { t, useT, type DictionaryStringKey } from "../../i18n";
import type { MaterialSummary, PracticePlan, PracticeQuestion } from "../../generated/bindings";
import "../../styles/practice.css";

const WIZARD_STEPS: Array<{ id: string; labelKey: DictionaryStringKey }> = [
  { id: "setup", labelKey: "practice.wizard.steps.setup" },
  { id: "style", labelKey: "practice.wizard.steps.style" },
  { id: "config", labelKey: "practice.wizard.steps.config" },
  { id: "preview", labelKey: "practice.wizard.steps.preview" },
];

// 注意：value 是后端契约（generatePracticePlan interviewerStyle），必须保持原样；
// 界面显示用 labelKey/hintKey 本地化。
const INTERVIEWER_STYLES = [
  { value: "面试官", labelKey: "practice.wizard.styles.interviewer.label", hintKey: "practice.wizard.styles.interviewer.hint" },
  { value: "HR", labelKey: "practice.wizard.styles.hr.label", hintKey: "practice.wizard.styles.hr.hint" },
  { value: "严苛面试官", labelKey: "practice.wizard.styles.strict.label", hintKey: "practice.wizard.styles.strict.hint" },
] as const;

type InterviewerStyleValue = (typeof INTERVIEWER_STYLES)[number]["value"];

// 同样：difficulty 值是后端契约，显示用 difficulties 字典。
const DIFFICULTIES = ["基础", "标准", "进阶"] as const;

type DifficultyValue = (typeof DIFFICULTIES)[number];

const DIFFICULTY_LABEL_KEYS: Record<DifficultyValue, DictionaryStringKey> = {
  基础: "practice.wizard.difficulties.basic",
  标准: "practice.wizard.difficulties.standard",
  进阶: "practice.wizard.difficulties.advanced",
};

function styleLabelKey(value: string): DictionaryStringKey | null {
  return INTERVIEWER_STYLES.find((style) => style.value === value)?.labelKey ?? null;
}

function difficultyLabel(value: string): string {
  const key = DIFFICULTY_LABEL_KEYS[value as DifficultyValue];
  return key ? t(key) : value;
}

const MIN_QUESTIONS = 1;
const MAX_QUESTIONS = 12;
/** 估算口径与后端提示词一致：每题回答约 3～5 分钟。 */
const MINUTES_PER_QUESTION: [number, number] = [3, 5];

interface PracticeWizardProps {
  /** 测试注入：跳过真实路由跳转。 */
  materialsLink?: string;
}

export function PracticeWizard({ materialsLink = "/materials" }: PracticeWizardProps = {}) {
  useT();
  // 应用外壳（app/shell.tsx）使用 wouter/use-hash-location（hash 路由）；
  // 这里必须用同一路由原语。此前误用 wouter 默认的 pathname 路由，
  // 开始训练后的 navigate("/") 会把地址推成无 hash 的 "/"，
  // hash 路由收不到任何通知，界面停留在向导页（lane-I I03 端到端红灯）。
  const [, navigate] = useHashLocation();
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
      setMessage(t("practice.wizard.ipcMaterialsFailed"));
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
      setMessage(t("practice.wizard.ipcGenerateFailed"));
    } finally {
      setBusy(false);
    }
  }

  async function handleSave(event: FormEvent) {
    event.preventDefault();
    if (!plan) return;
    if (!planEditable) {
      setMessage(t("practice.wizard.planInvalid"));
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
      setMessage(t("practice.wizard.ipcSaveFailed"));
    } finally {
      setBusy(false);
    }
  }

  // 开始训练：训练按已保存的题单 id 启动，随后跳到工作台的训练界面。
  async function handleStartTraining() {
    if (!plan) return;
    if (!planEditable) {
      setMessage(t("practice.wizard.planInvalid"));
      return;
    }
    setBusy(true);
    setMessage("");
    try {
      if (!saved) {
        const saveResult = await api.savePracticePlan(plan);
        if (!saveResult.ok) {
          setMessage(errorText(saveResult.error));
          return;
        }
        setPlan(saveResult.data);
        setSaved(true);
      }
      const result = await api.startPracticeSession({ planId: plan.id });
      if (!result.ok) {
        setMessage(errorText(result.error));
        return;
      }
      if (result.data.kind === "blocked") {
        setMessage(result.data.issues[0]?.code ?? t("practice.wizard.notReadyFallback"));
        return;
      }
      navigate("/");
    } catch {
      setMessage(t("practice.wizard.ipcStartFailed"));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="service-panel practice-wizard" aria-labelledby="practice-wizard-heading">
      <div className="library-heading">
        <h2 id="practice-wizard-heading">{t("practice.wizard.heading")}</h2>
        <span className="muted">{t("practice.wizard.stepCounter", { n: step + 1, total: WIZARD_STEPS.length })}</span>
      </div>
      <ol className="practice-steps" aria-label={t("practice.wizard.stepsAria")}>
        {WIZARD_STEPS.map(({ id, labelKey }, index) => (
          <li
            key={id}
            data-current={index === step ? "true" : undefined}
            data-done={index < step ? "true" : undefined}
            aria-current={index === step ? "step" : undefined}
          >
            {t(labelKey)}
          </li>
        ))}
      </ol>
      {message && (
        <p className="services-message" role="status">
          {message}
        </p>
      )}

      {step === 0 && (
        <div className="practice-step-panel" aria-label={t("practice.wizard.steps.setup")}>
          <form className="service-form" onSubmit={(event) => { event.preventDefault(); if (positionValid) setStep(1); }}>
            <label>
              {t("practice.wizard.positionLabel")}
              <input
                value={position}
                placeholder={t("practice.wizard.positionPlaceholder")}
                maxLength={80}
                onChange={(event) => setPosition(event.target.value)}
              />
            </label>
            {materialsState === "loading" && <p className="muted" role="status">{t("practice.wizard.materialsLoading")}</p>}
            {materialsState === "error" && (
              <p className="muted">{t("practice.wizard.materialsError")}</p>
            )}
            {materialsState === "ready" && materials.length === 0 && (
              <EmptyState
                className="practice-empty"
                icon={<FolderOpen size={28} aria-hidden="true" />}
                title={t("practice.wizard.emptyMaterialsTitle")}
                hint={t("practice.wizard.emptyMaterialsHint")}
              />
            )}
            {materials.length > 0 && (
              <>
                <label>
                  {t("practice.wizard.jdLabel")}
                  <select value={jdMaterialId} onChange={(event) => setJdMaterialId(event.target.value)}>
                    <option value="">{t("practice.wizard.notUsed")}</option>
                    {materials.map((item) => (
                      <option key={item.id} value={item.id}>{item.fileName}</option>
                    ))}
                  </select>
                </label>
                <label>
                  {t("practice.wizard.resumeLabel")}
                  <select value={resumeMaterialId} onChange={(event) => setResumeMaterialId(event.target.value)}>
                    <option value="">{t("practice.wizard.notUsed")}</option>
                    {materials.map((item) => (
                      <option key={item.id} value={item.id}>{item.fileName}</option>
                    ))}
                  </select>
                </label>
                <p className="muted practice-hint">
                  {t("practice.wizard.materialsHintBefore")}<Link href={materialsLink}>{t("practice.wizard.materialsCtaLink")}</Link>{t("practice.wizard.materialsHintAfter")}
                </p>
              </>
            )}
            {materials.length === 0 && materialsState === "ready" && (
              <p className="muted practice-hint">
                <Link href={materialsLink}>{t("practice.wizard.materialsCtaLink")}</Link>{t("practice.wizard.materialsDeferredHintAfter")}
              </p>
            )}
            <div className="service-actions practice-nav">
              <span />
              <button className="button-primary" type="submit" disabled={!positionValid}>
                {t("practice.wizard.next")}
              </button>
            </div>
          </form>
        </div>
      )}

      {step === 1 && (
        <div className="practice-step-panel" aria-label={t("practice.wizard.steps.style")}>
          <fieldset className="practice-style-group">
            <legend>{t("practice.wizard.stylesLegend")}</legend>
            {INTERVIEWER_STYLES.map((style) => (
              <label key={style.value} className="practice-style-option">
                <input
                  type="radio"
                  name="practice-interviewer-style"
                  value={style.value}
                  checked={interviewerStyle === style.value}
                  onChange={() => setInterviewerStyle(style.value)}
                />
                <span className="practice-style-name">{t(style.labelKey)}</span>
                <span className="muted">{t(style.hintKey)}</span>
              </label>
            ))}
          </fieldset>
          <div className="service-actions practice-nav">
            <button className="button-ghost" type="button" onClick={() => setStep(0)}>
              {t("practice.wizard.prev")}
            </button>
            <button className="button-primary" type="button" onClick={() => setStep(2)}>
              {t("practice.wizard.next")}
            </button>
          </div>
        </div>
      )}

      {step === 2 && (
        <div className="practice-step-panel" aria-label={t("practice.wizard.steps.config")}>
          <form className="service-form" onSubmit={(event) => { event.preventDefault(); setStep(3); }}>
            <label>
              {t("practice.wizard.questionCountLabel", { max: MAX_QUESTIONS })}
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
              {t("practice.wizard.difficultyLabel")}
              <select value={difficulty} onChange={(event) => setDifficulty(event.target.value)}>
                {DIFFICULTIES.map((item) => (
                  <option key={item} value={item}>{difficultyLabel(item)}</option>
                ))}
              </select>
            </label>
            <p className="muted practice-hint">
              {t("practice.wizard.estimatedHint", { range: estimatedMinutes })}
            </p>
            <div className="service-actions practice-nav">
              <button className="button-ghost" type="button" onClick={() => setStep(1)}>
                {t("practice.wizard.prev")}
              </button>
              <button className="button-primary" type="submit">
                {t("practice.wizard.next")}
              </button>
            </div>
          </form>
        </div>
      )}

      {step === 3 && (
        <div className="practice-step-panel" aria-label={t("practice.wizard.steps.preview")}>
          {!plan && (
            <div className="practice-generate">
              <p>
                {t("practice.wizard.positionPrefix")}<strong>{position.trim()}</strong>
                <span className="muted">{t("practice.wizard.styleSummary", { style: styleLabelKey(interviewerStyle) ? t(styleLabelKey(interviewerStyle)!) : interviewerStyle, difficulty: difficultyLabel(difficulty), n: questionCount })}</span>
              </p>
              <button className="button-primary" type="button" disabled={busy} onClick={() => void handleGenerate()}>
                <Sparkles size={16} aria-hidden="true" />
                {busy ? t("practice.wizard.generating") : t("practice.wizard.generate")}
              </button>
              <p className="muted">{t("practice.wizard.generateNote")}</p>
            </div>
          )}
          {plan && (
            <form className="practice-plan-editor" onSubmit={handleSave}>
              <div className="library-heading">
                <h3>
                  <ClipboardList size={16} aria-hidden="true" />
                  {plan.title}
                </h3>
                <span className="muted">{t("practice.wizard.questionCountSuffix", { n: plan.questions.length })}</span>
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
                  {t("practice.wizard.regenerate")}
                </button>
                <span className="practice-nav-group">
                  <button
                    className="button-ghost"
                    type="button"
                    disabled={busy || !planEditable}
                    onClick={() => void handleStartTraining()}
                  >
                    <Play size={14} aria-hidden="true" />
                    {t("practice.wizard.startTraining")}
                  </button>
                  <button className="button-primary" type="submit" disabled={busy || !planEditable}>
                    {saved ? t("practice.wizard.saved") : t("practice.wizard.save")}
                  </button>
                </span>
              </div>
              {saved && <p className="muted" role="status">{t("practice.wizard.savedNote")}</p>}
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
        <span className="muted">{t("practice.wizard.questionLabel", { n: index + 1 })}</span>
        {question.focus && <span className="status-badge">{question.focus}</span>}
        <button className="button-ghost practice-question-remove" type="button" disabled={busy} aria-label={t("practice.wizard.deleteQuestionAria", { n: index + 1 })} onClick={onRemove}>
          <Trash2 size={14} aria-hidden="true" />
          {t("practice.wizard.delete")}
        </button>
      </div>
      <label className="practice-question-prompt">
        <span className="muted">{t("practice.wizard.questionPromptLabel", { n: index + 1 })}</span>
        <textarea
          value={question.prompt}
          rows={2}
          maxLength={500}
          onChange={(event) => onPromptChange(event.target.value)}
        />
      </label>
      {question.expectedPoints.length > 0 && (
        <p className="muted practice-question-points">{t("practice.wizard.expectedPoints", { points: question.expectedPoints.join("、") })}</p>
      )}
      {question.followups.length > 0 && (
        <p className="muted practice-question-points">{t("practice.wizard.followups", { points: question.followups.join("、") })}</p>
      )}
    </li>
  );
}
