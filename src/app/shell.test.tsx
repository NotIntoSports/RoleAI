import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AVATAR_STORAGE_KEY } from "../features/session/avatar-preference";
import { Shell } from "./shell";

vi.mock("../api/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../api/commands")>()),
  getLegacyMigrationStatus: vi.fn().mockResolvedValue({
    ok: true,
    data: { applied: false, reenterSecrets: false, omitted: [] },
  }),
  getConfigPublic: vi.fn().mockResolvedValue({
    ok: true,
    data: {
      configVersion: 1,
      application: { locale: null },
      models: { providers: [], activeProviderId: null },
      speech: { voiceRoutes: [], activeVoiceRouteId: null },
      knowledge: { embeddingConfigs: [], activeEmbeddingConfigId: null },
      storage: { exportDirectory: null },
      roleProfiles: [],
      activeRoleProfileId: null,
      diagnostics: { logRetentionDays: 14 },
    },
  }),
  listMaterials: vi.fn().mockResolvedValue({ ok: true, data: [] }),
}));

describe("Shell", () => {
  beforeEach(() => {
    window.history.replaceState(null, "", "/");
  });
  afterEach(() => {
    cleanup();
    window.localStorage.removeItem(AVATAR_STORAGE_KEY);
  });

  it("renders navigation and default workspace page", () => {
    render(<Shell />);
    expect(screen.getByRole("navigation", { name: "主导航" })).toBeTruthy();
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("工作台");
  });

  it("renders records page when hash is #/records", () => {
    window.location.hash = "#/records";
    render(<Shell />);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("记录");
  });

  it("renders materials page when hash is #/materials", () => {
    window.location.hash = "#/materials";
    render(<Shell />);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("资料");
  });

  it("renders settings page when hash is #/settings", () => {
    window.location.hash = "#/settings";
    render(<Shell />);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("设置");
  });

  it("clicking nav button changes the displayed page", () => {
    render(<Shell />);
    const recordsButton = screen.getByRole("button", { name: "记录" });
    fireEvent.click(recordsButton);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("记录");
  });

  it("drops a deep link's ?category= when navigating from the sidebar", () => {
    window.history.replaceState(null, "", "/?category=routes#/workspace");
    render(<Shell />);
    fireEvent.click(screen.getByRole("button", { name: "设置" }));
    expect(window.location.search).toBe("");
    expect(window.location.hash).toBe("#/settings");
    expect(document.getElementById("settings-panel-providers")?.hidden).toBe(false);
    expect(document.getElementById("settings-panel-routes")?.hidden).toBe(true);
  });

  it("follows an in-page link to the settings category it names", async () => {
    window.localStorage.setItem(AVATAR_STORAGE_KEY, "on");
    render(<Shell />);
    const link = await screen.findByRole("link", { name: "去选择形象" });
    expect(link.getAttribute("href")).toBe("#/settings?category=roles");
    fireEvent.click(link);
    expect(window.location.hash).toBe("#/settings");
    expect(window.location.search).toBe("?category=roles");
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("设置");
    expect(document.getElementById("settings-panel-roles")?.hidden).toBe(false);
  });

  it("starts the new page at the top after navigating from a long form", () => {
    window.location.hash = "#/settings";
    render(<Shell />);
    const main = screen.getByRole("main");
    main.scrollTop = 400;
    fireEvent.click(screen.getByRole("button", { name: "工作台" }));
    expect(main.scrollTop).toBe(0);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("工作台");
  });
});
