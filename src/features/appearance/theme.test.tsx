import { act, cleanup, renderHook } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  THEME_STORAGE_KEY,
  initializeTheme,
  setThemePreference,
  useThemePreference,
} from "./theme";

let restoreMatchMedia: (() => void) | undefined;

// jsdom 未实现 matchMedia，用 defineProperty 注入可控实现。
function mockMatchMedia(matchesDark: boolean) {
  const impl = vi.fn((query: string) => ({
    matches: matchesDark,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  }) as MediaQueryList);
  Object.defineProperty(window, "matchMedia", { configurable: true, writable: true, value: impl });
  restoreMatchMedia = () => {
    Reflect.deleteProperty(window, "matchMedia");
    restoreMatchMedia = undefined;
  };
  return impl;
}

describe("theme preference", () => {
  let dispose: (() => void) | undefined;

  beforeEach(() => {
    window.localStorage.clear();
  });

  afterEach(() => {
    dispose?.();
    dispose = undefined;
    restoreMatchMedia?.();
    cleanup();
    vi.restoreAllMocks();
    delete document.documentElement.dataset.theme;
    document.documentElement.style.colorScheme = "";
  });

  it("falls back to system resolution for missing or invalid stored values", () => {
    const media = mockMatchMedia(true);
    dispose = initializeTheme();
    expect(media).toHaveBeenCalledWith("(prefers-color-scheme: dark)");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.style.colorScheme).toBe("dark");
    dispose();

    window.localStorage.setItem(THEME_STORAGE_KEY, "banana");
    dispose = initializeTheme();
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("persists the choice to localStorage and notifies subscribers", () => {
    const { result } = renderHook(() => useThemePreference());
    expect(result.current).toBe("system");
    act(() => setThemePreference("dark"));
    expect(result.current).toBe("dark");
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe("dark");
    act(() => setThemePreference("light"));
    expect(result.current).toBe("light");
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe("light");
  });

  it("initializeTheme applies explicit light and dark overrides over the media query", () => {
    mockMatchMedia(true);
    window.localStorage.setItem(THEME_STORAGE_KEY, "light");
    dispose = initializeTheme();
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(document.documentElement.style.colorScheme).toBe("light");
    dispose();
    window.localStorage.setItem(THEME_STORAGE_KEY, "dark");
    dispose = initializeTheme();
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.style.colorScheme).toBe("dark");
  });

  it("keeps the in-memory preference when localStorage writes fail", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("quota");
    });
    const { result } = renderHook(() => useThemePreference());
    expect(() => act(() => setThemePreference("dark"))).not.toThrow();
    expect(result.current).toBe("dark");
  });
});
