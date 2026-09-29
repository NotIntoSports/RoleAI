import { cleanup, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, describe, expect, it } from "vitest";

import { ErrorNotice, errorNoticeText } from "./error-notice";

describe("errorNoticeText", () => {
  it("formats code：message", () => {
    expect(errorNoticeText({ code: "SOMETHING_FAILED", message: "原始信息" })).toBe(
      "SOMETHING_FAILED：原始信息",
    );
  });

  it("prefixes the field when present", () => {
    expect(errorNoticeText({ code: "SOMETHING_FAILED", message: "原始信息", field: "voiceRouteId" })).toBe(
      "voiceRouteId：SOMETHING_FAILED：原始信息",
    );
    expect(errorNoticeText({ code: "SOMETHING_FAILED", message: "原始信息", field: null })).toBe(
      "SOMETHING_FAILED：原始信息",
    );
  });
});

describe("ErrorNotice", () => {
  afterEach(cleanup);

  it("renders nothing for a null error", () => {
    const { container } = render(<ErrorNotice error={null} />);
    expect(container.innerHTML).toBe("");
  });

  it("renders the fallback format with role=status, matching existing pages", () => {
    render(<ErrorNotice error={{ code: "SOMETHING_FAILED", message: "原始信息" }} />);
    const notice = screen.getByRole("status");
    expect(notice.textContent).toBe("SOMETHING_FAILED：原始信息");
    expect(notice.className).toBe("services-message");
  });
});
