import { expect, test, type Page } from "@playwright/test";

/**
 * 在线演示版冒烟用例：六个页面、完整脚本会话、刷新保留、一键重置。
 * 共享一个 dev server 的 localStorage（workers=1 串行），用例按顺序构成一条用户旅程。
 */

const DEMO_PATH = "/RoleAI/demo/";

async function open(page: Page, hash = "") {
  // hash 形如 "#/materials"；DEMO_PATH 已以 "/" 结尾，不能再拼一个斜杠
  // （双斜杠会绕过 dev server 的 demo.html 改写，回退到正式入口）。
  await page.goto(`${DEMO_PATH}${hash}`);
  await expect(page.getByRole("navigation", { name: "主导航" })).toBeVisible();
}

/** 跳过首次引导（或先走一遍引导再关闭，覆盖引导本身）。 */
async function dismissGuideIfVisible(page: Page) {
  const dialog = page.getByRole("dialog", { name: "在线演示引导" });
  if (await dialog.isVisible().catch(() => false)) {
    await page.getByRole("button", { name: "跳过引导" }).click();
  }
  await expect(dialog).toHaveCount(0);
}

test("first visit shows onboarding, and all six pages open", async ({ page }) => {
  await open(page);
  const dialog = page.getByRole("dialog", { name: "在线演示引导" });
  await expect(dialog).toBeVisible();
  await page.getByRole("button", { name: "下一步" }).click();
  await page.getByRole("button", { name: "下一步" }).click();
  await page.getByRole("button", { name: "开始体验" }).click();
  await expect(dialog).toHaveCount(0);

  const pages: Array<[string, string]> = [
    ["#/livestream", "虚拟直播"],
    ["#/materials", "资料"],
    ["#/records", "记录"],
    ["#/services", "服务"],
    ["#/settings", "设置"],
    ["#/workspace", "工作台"],
  ];
  for (const [hash, heading] of pages) {
    await open(page, hash);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText(heading);
  }
});

test("runs one full scripted session, then sees it in records", async ({ page }) => {
  await open(page, "#/workspace");
  await dismissGuideIfVisible(page);
  // 横幅在页面上（截图之外的常规 UX 的一部分）。
  await expect(page.getByRole("note", { name: "演示模式说明" })).toBeVisible();

  await page.getByRole("button", { name: "开始语音会话" }).click();
  await expect(page.getByRole("button", { name: "结束通话" })).toBeVisible();

  // 第 1 轮用户发言逐字上屏。
  await expect(page.getByText("面试官您好，我最有代表性的项目", { exact: false }).first()).toBeVisible();
  // 回答流式上屏并落库（出现在历史气泡里）。
  await expect(
    page.getByText("已读回执你们是怎么存储的", { exact: false }).first(),
  ).toBeVisible({ timeout: 30_000 });
  // 第 3 轮打断后的追问也播出来，证明打断演示完成。
  await expect(page.getByText("不好意思打断一下", { exact: false }).first()).toBeVisible({
    timeout: 60_000,
  });
  // 收尾轮（脚本第 4 轮回答）播完。
  await expect(page.getByText("主动打断并给出自己的论据", { exact: false }).first()).toBeVisible({
    timeout: 30_000,
  });

  await page.getByRole("button", { name: "结束通话" }).click();
  await expect(page.getByRole("button", { name: "开始语音会话" })).toBeVisible();

  // 每个用例是独立的浏览器上下文：本上下文里是 4 条种子记录 + 1 条新会话。
  await open(page, "#/records");
  await expect(page.getByText(/次会话/)).toHaveText(/5 次会话/);
  await expect(page.getByText(/session-demo-/, { exact: false }).first()).toBeVisible();
});

test("data survives a page refresh (records and imported material)", async ({ page }) => {
  // 导入一份资料。
  await open(page, "#/materials");
  await dismissGuideIfVisible(page);
  await page.getByText("导入资料").click();
  await page.getByPlaceholder("输入本地文件的完整路径").fill("D:\\演示\\候选人简历（虚构）.pdf");
  await page.getByRole("button", { name: "导入", exact: true }).click();
  await expect(page.getByText("候选人简历（虚构）.pdf")).toBeVisible();
  await expect(page.getByText(/份资料/)).toHaveText(/4 份资料/);

  await page.reload();
  await expect(page.getByRole("navigation", { name: "主导航" })).toBeVisible();
  await expect(page.getByText(/份资料/)).toHaveText(/4 份资料/);
  await expect(page.getByText("候选人简历（虚构）.pdf")).toBeVisible();

  // 记录页同样在刷新后可用（4 条种子记录）。
  await open(page, "#/records");
  await expect(page.getByText(/次会话/)).toHaveText(/4 次会话/);
});

test("demo reset restores seed data", async ({ page }) => {
  await open(page, "#/materials");
  await dismissGuideIfVisible(page);
  // __roleaiDemoReset 会自行刷新页面，这里等待刷新完成后再断言。
  await Promise.all([
    page.waitForURL(/RoleAI\/demo\//),
    page.evaluate(() => {
      (window as { __roleaiDemoReset?: () => void }).__roleaiDemoReset?.();
    }),
  ]);
  await expect(page.getByRole("navigation", { name: "主导航" })).toBeVisible();
  await expect(page.getByText(/份资料/)).toHaveText(/3 份资料/);
});
