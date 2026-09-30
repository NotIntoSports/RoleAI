import { useSyncExternalStore } from "react";

import { default as enDictionary } from "../locales/en";
import { default as zhCN, type Dictionary } from "../locales/zh-CN";

export type Language = "zh-CN" | "en";
export type LanguagePreference = Language | "system";
export const LANGUAGE_STORAGE_KEY = "ai-assistant.language";

const dictionaries: Record<Language, Dictionary> = {
  "zh-CN": zhCN,
  en: enDictionary,
};

// 从字典类型推导全部点号路径键；键名拼错会在编译期报错。
export type MessageKey<T> = T extends string
  ? never
  : { [K in keyof T & string]: T[K] extends string ? K : `${K}.${MessageKey<T[K]>}` }[keyof T & string];

export type DictionaryKey = MessageKey<Dictionary>;
export type TParams = Record<string, string | number>;

const listeners = new Set<() => void>();

function parsePreference(value: string | null): LanguagePreference {
  return value === "zh-CN" || value === "en" ? value : "system";
}

function readPreference(): LanguagePreference {
  try { return parsePreference(window.localStorage.getItem(LANGUAGE_STORAGE_KEY)); }
  catch { return "system"; }
}

function systemLanguage(): Language {
  try {
    return window.navigator.language?.toLowerCase().startsWith("zh") ? "zh-CN" : "en";
  } catch {
    // 无法检测系统语言时回退到基线语言中文。
    return "zh-CN";
  }
}

export function resolveLanguage(preference: LanguagePreference): Language {
  return preference === "system" ? systemLanguage() : preference;
}

let preference = readPreference();
let resolved = resolveLanguage(preference);

function applyDocumentLanguage() {
  try { document.documentElement.lang = resolved; } catch { /* 非 DOM 环境忽略。 */ }
}

function notify(next: LanguagePreference) {
  preference = next;
  resolved = resolveLanguage(next);
  listeners.forEach((listener) => listener());
}

export function setLanguagePreference(next: LanguagePreference) {
  // 存储不可用时仍允许本次窗口切换语言。
  try { window.localStorage.setItem(LANGUAGE_STORAGE_KEY, next); } catch { /* 保留内存偏好。 */ }
  notify(next);
}

export function parseLanguagePreference(value: string): LanguagePreference {
  return parsePreference(value);
}

export function initializeLanguage() {
  notify(readPreference());
  applyDocumentLanguage();
  const onStorage = (event: StorageEvent) => {
    if (event.key === LANGUAGE_STORAGE_KEY || event.key === null) notify(parsePreference(event.newValue));
  };
  listeners.add(applyDocumentLanguage);
  window.addEventListener("storage", onStorage);
  return () => {
    listeners.delete(applyDocumentLanguage);
    window.removeEventListener("storage", onStorage);
  };
}

function format(template: string, params?: TParams): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) =>
    Object.hasOwn(params, name) ? String(params[name]) : match,
  );
}

function lookup(dictionary: Dictionary, key: string): string | undefined {
  let node: unknown = dictionary;
  for (const part of key.split(".")) {
    if (typeof node !== "object" || node === null) return undefined;
    node = (node as Record<string, unknown>)[part];
  }
  return typeof node === "string" ? node : undefined;
}

function reportMissing(key: string) {
  // 开发模式报错；生产模式静默（由回退逻辑兜底）。
  if (import.meta.env.DEV) console.error(`[i18n] missing key: ${key}`);
}

export function translateText(language: Language, key: DictionaryKey, params?: TParams): string {
  const localized = lookup(dictionaries[language], key);
  if (localized !== undefined) return format(localized, params);
  reportMissing(key);
  // 当前语言缺键时回退中文基线；两者都缺时返回键名本身。
  const fallback = language === "zh-CN" ? undefined : lookup(dictionaries["zh-CN"], key);
  return fallback !== undefined ? format(fallback, params) : key;
}

export function t(key: DictionaryKey, params?: TParams): string {
  return translateText(resolved, key, params);
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

function getResolvedLanguage(): Language {
  return resolved;
}

function getServerLanguage(): Language {
  return "zh-CN";
}

export function useLanguage(): Language {
  return useSyncExternalStore(subscribe, getResolvedLanguage, getServerLanguage);
}

export function useLanguagePreference(): LanguagePreference {
  return useSyncExternalStore(subscribe, () => preference, () => "system" as const);
}

export function useT() {
  useLanguage();
  return t;
}
