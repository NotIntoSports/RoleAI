import { cleanup, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, describe, expect, it } from "vitest";

import { EmptyState } from "./empty-state";

describe("EmptyState", () => {
  afterEach(cleanup);

  it("renders a bare title as p.empty-state", () => {
    const { container } = render(<EmptyState title="还没有角色。" />);
    const node = screen.getByText("还没有角色。");
    expect(node.tagName).toBe("P");
    expect(node.className).toBe("empty-state");
    expect(container.querySelector("span.muted")).toBeNull();
  });

  it("renders title with hint and icon in the rich variant", () => {
    const { container } = render(
      <EmptyState
        className="library-empty"
        icon={<i data-testid="icon" aria-hidden="true" />}
        title="还没有记录。"
        hint="在工作台开始会话后，可在这里回看对话。"
      />,
    );
    const box = container.querySelector("div.empty-state.library-empty");
    expect(box).toBeTruthy();
    expect(box?.querySelector("p")?.textContent).toBe("还没有记录。");
    expect(box?.querySelector("span.muted")?.textContent).toBe(
      "在工作台开始会话后，可在这里回看对话。",
    );
    expect(screen.getByTestId("icon")).toBeTruthy();
  });

  it("hint alone also uses the rich variant", () => {
    const { container } = render(<EmptyState title="没有内容" hint="提示" />);
    expect(container.querySelector("div.empty-state")).toBeTruthy();
    expect(screen.getByText("提示").className).toBe("muted");
  });
});
