import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { setLanguagePreference } from "../../i18n";
import { PracticeWizard } from "./practice-wizard";

describe("practice wizard english copy", () => {
  afterEach(cleanup);

  it("renders localized step names and interviewer styles in English", async () => {
    try {
      act(() => setLanguagePreference("en"));
      render(<PracticeWizard />);
      expect(screen.getByRole("heading", { name: "Session prep" })).toBeTruthy();
      expect(screen.getByText("Role and materials")).toBeTruthy();
      expect(screen.getByText("Target role")).toBeTruthy();
    } finally {
      act(() => setLanguagePreference("system"));
    }
  });
});
