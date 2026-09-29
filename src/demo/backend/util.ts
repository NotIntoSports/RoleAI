// 演示后端共用工具：结果包装、模拟延迟、ID 生成、bigint 感知的 JSON 序列化。
import type { CommandResult, PublicError } from "../../generated/bindings";

export function ok<T>(data: T): CommandResult<T> {
  return { ok: true, data };
}

export function err(code: string, message: string): CommandResult<never> {
  return { ok: false, error: publicError(code, message) };
}

export function publicError(code: string, message: string): PublicError {
  return { code, message, requestId: "demo", retryable: false };
}

/** sleep(minMs..maxMs) 的均匀随机延迟，让演示有真实操作的手感。 */
export function latency(minMs: number, maxMs: number): Promise<void> {
  const ms = minMs + Math.floor(Math.random() * Math.max(1, maxMs - minMs + 1));
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export function demoId(prefix: string): string {
  const rand =
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID().slice(0, 8)
      : Math.random().toString(36).slice(2, 10);
  return `${prefix}-${rand}`;
}

const BIGINT_TAG = "__demoBigInt__:";

export function stringifyState(value: unknown): string {
  return JSON.stringify(value, (_key, item) =>
    typeof item === "bigint" ? `${BIGINT_TAG}${item.toString()}` : item,
  );
}

export function parseState<T>(raw: string): T {
  return JSON.parse(raw, (_key, item) =>
    typeof item === "string" && item.startsWith(BIGINT_TAG)
      ? BigInt(item.slice(BIGINT_TAG.length))
      : item,
  ) as T;
}
