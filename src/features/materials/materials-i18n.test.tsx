import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import { setLanguagePreference } from "../../i18n";
import { MaterialsLibrary } from "./materials-library";

vi.mock("../../api/commands", () => ({
  listMaterials: vi.fn(),
  importMaterial: vi.fn(),
  searchMaterials: vi.fn(),
  deleteMaterial: vi.fn(),
  indexMaterials: vi.fn(),
}));

describe("materials library english copy", () => {
  beforeEach(() => {
    vi.mocked(commands.listMaterials).mockResolvedValue({ ok: true, data: [] });
  });
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("renders localized headings and actions in English", async () => {
    try {
      act(() => setLanguagePreference("en"));
      render(<MaterialsLibrary />);
      expect(screen.getByRole("heading", { name: "Materials library" })).toBeTruthy();
      expect(screen.getByRole("button", { name: "Rebuild index" })).toBeTruthy();
      expect(screen.getByRole("button", { name: "Import" })).toBeTruthy();
      expect(await screen.findByText("No materials yet.")).toBeTruthy();
      expect(screen.getByText("0 materials")).toBeTruthy();
    } finally {
      act(() => setLanguagePreference("system"));
    }
  });
});
