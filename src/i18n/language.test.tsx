import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AppearanceSettings } from "../features/appearance/appearance-settings";
import { en } from "../locales/en";
import { I18nProvider } from "./I18nProvider";
import { initializeLanguage, LANGUAGE_STORAGE_KEY, setLanguagePreference, t, useLanguage, useT } from "./language";

function stubSystemLanguage(language: string) {
  Object.defineProperty(window.navigator, "language", { value: language, configurable: true, writable: false });
}

function Probe() {
  const translate = useT();
  const language = useLanguage();
  return (
    <p>
      <span data-testid="language">{language}</span>
      <span data-testid="label">{translate("settings.appearance.language.legend")}</span>
    </p>
  );
}

describe("i18n language infrastructure", () => {
  let dispose: (() => void) | undefined;

  beforeEach(() => {
    window.localStorage.clear();
    stubSystemLanguage("zh-CN");
  });

  afterEach(() => {
    cleanup();
    dispose?.();
    dispose = undefined;
    vi.restoreAllMocks();
    vi.unstubAllEnvs();
    stubSystemLanguage("zh-CN");
  });

  it("follows the system language by default and reflects it on the document", () => {
    dispose = initializeLanguage();
    render(<Probe />);
    expect(screen.getByTestId("language").textContent).toBe("zh-CN");
    expect(document.documentElement.lang).toBe("zh-CN");
    expect(screen.getByTestId("label").textContent).toBe("界面语言");
    expect(window.localStorage.getItem(LANGUAGE_STORAGE_KEY)).toBeNull();
  });

  it("resolves non-Chinese system languages to English", () => {
    stubSystemLanguage("en-US");
    dispose = initializeLanguage();
    render(<Probe />);
    expect(screen.getByTestId("language").textContent).toBe("en");
    expect(screen.getByTestId("label").textContent).toBe("Interface language");
    expect(document.documentElement.lang).toBe("en");
  });

  it("switches UI text immediately and restores the saved preference on restart", () => {
    dispose = initializeLanguage();
    render(<Probe />);
    act(() => setLanguagePreference("en"));
    expect(screen.getByTestId("label").textContent).toBe("Interface language");
    expect(document.documentElement.lang).toBe("en");
    expect(window.localStorage.getItem(LANGUAGE_STORAGE_KEY)).toBe("en");
    dispose();
    dispose = undefined;
    act(() => { dispose = initializeLanguage(); });
    expect(screen.getByTestId("language").textContent).toBe("en");
    expect(screen.getByTestId("label").textContent).toBe("Interface language");
  });

  it("synchronizes language changes from another window", () => {
    dispose = initializeLanguage();
    render(<Probe />);
    act(() => window.dispatchEvent(new StorageEvent("storage", { key: LANGUAGE_STORAGE_KEY, newValue: "en" })));
    expect(screen.getByTestId("language").textContent).toBe("en");
    expect(screen.getByTestId("label").textContent).toBe("Interface language");
  });

  it("keeps working when storage is unavailable", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("unavailable"); });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("unavailable"); });
    dispose = initializeLanguage();
    render(<Probe />);
    act(() => setLanguagePreference("en"));
    expect(screen.getByTestId("label").textContent).toBe("Interface language");
  });

  it("reports missing keys in development and falls back to Chinese", () => {
    dispose = initializeLanguage();
    act(() => setLanguagePreference("en"));
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const original = en.settings.appearance.language.legend;
    delete (en.settings.appearance.language as { legend?: string }).legend;
    try {
      expect(t("settings.appearance.language.legend")).toBe("界面语言");
      expect(errorSpy).toHaveBeenCalledTimes(1);
    } finally {
      (en.settings.appearance.language as { legend: string }).legend = original;
    }
  });

  it("silently falls back to Chinese in production", () => {
    vi.stubEnv("DEV", false);
    dispose = initializeLanguage();
    act(() => setLanguagePreference("en"));
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const original = en.settings.appearance.language.english;
    delete (en.settings.appearance.language as { english?: string }).english;
    try {
      expect(t("settings.appearance.language.english")).toBe("English");
      expect(errorSpy).not.toHaveBeenCalled();
    } finally {
      (en.settings.appearance.language as { english: string }).english = original;
    }
  });

  it("switches the interface language from the appearance settings", () => {
    dispose = initializeLanguage();
    render(<AppearanceSettings />);
    expect(screen.getByRole("combobox", { name: "界面语言" })).toBeTruthy();
    fireEvent.change(screen.getByRole("combobox", { name: "界面语言" }), { target: { value: "en" } });
    expect(screen.getByRole("combobox", { name: "Interface language" })).toBeTruthy();
    expect(window.localStorage.getItem(LANGUAGE_STORAGE_KEY)).toBe("en");
  });

  it("initializes the language when the provider mounts", () => {
    render(
      <I18nProvider>
        <Probe />
      </I18nProvider>,
    );
    expect(screen.getByTestId("language").textContent).toBe("zh-CN");
    expect(document.documentElement.lang).toBe("zh-CN");
  });
});
