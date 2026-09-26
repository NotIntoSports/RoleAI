import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import { RecordsPage } from "./records-page";

vi.mock("../../api/commands", () => ({
  listSessions: vi.fn(),
  getSession: vi.fn(),
  exportSession: vi.fn(),
  deleteSession: vi.fn(),
  getConfigPublic: vi.fn(),
}));

describe("RecordsPage", () => {
  beforeEach(() => {
    vi.mocked(commands.listSessions).mockResolvedValue({ ok: true, data: [] });
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
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
    });
  });
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("renders the records heading", () => {
    render(<RecordsPage />);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toContain("记录");
  });

  it("renders the records region and its empty state", async () => {
    render(<RecordsPage />);
    expect(screen.getByRole("region", { name: "会话记录" })).toBeTruthy();
    expect(await screen.findByText("还没有记录。")).toBeTruthy();
    expect(screen.queryByText(/Design §/)).toBeNull();
  });

  it("does not call fetch", () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch");
    render(<RecordsPage />);
    expect(fetchSpy).not.toHaveBeenCalled();
    fetchSpy.mockRestore();
  });

  it("does not contain /api/ paths", () => {
    const { container } = render(<RecordsPage />);
    expect(container.innerHTML).not.toMatch(/\/api\//);
  });

  it("includes the records list and drops the leftover placeholder", async () => {
    render(<RecordsPage />);
    expect(await screen.findByRole("heading", { name: "会话记录" })).toBeTruthy();
    expect(screen.queryByText(/尚未接入业务逻辑/)).toBeNull();
  });
});
