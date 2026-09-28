export interface CommandErrorView {
  code: string;
  message: string;
  field?: string | null;
}

// 与各页面原有 errorText 回退模板逐字一致：`field：CODE：message`。
export function errorNoticeText(error: CommandErrorView): string {
  return `${error.field ? error.field + "：" : ""}${error.code}：${error.message}`;
}

export function ErrorNotice({ error }: { error: CommandErrorView | null }) {
  if (!error) return null;
  return <p className="services-message" role="status">{errorNoticeText(error)}</p>;
}
