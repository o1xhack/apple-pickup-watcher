import { type Target, targetKey } from "./types";
import { invoke } from "@tauri-apps/api/core";

/** 与 Rust 的目标版本摘要一致，避免 Mac 与手机同时编辑时覆盖彼此。 */
export async function targetRevision(targets: Target[]): Promise<string> {
  if (!globalThis.crypto?.subtle) return invoke<string>("target_revision", { targets });
  const ordered = [...targets].sort((a, b) => targetKey(a) < targetKey(b) ? -1 : targetKey(a) > targetKey(b) ? 1 : 0);
  const canonical = ordered.map(t => [t.locale, t.storeNumber, t.partNumber, t.criticalAlert ?? false, t.pickupLocation ?? null]);
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(JSON.stringify(canonical)));
  return [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, "0")).join("");
}
