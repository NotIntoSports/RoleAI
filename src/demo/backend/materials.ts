// 资料库命令：material_list / import / search / delete / index。
// 检索用关键词匹配打分（长词加权），返回 bindings 的 MaterialSearchHit。
import type { MaterialSearchHit, MaterialSummary } from "../../generated/bindings";

import type { DemoMaterialDoc } from "./materials-data";
import { demoT } from "./demo-text";
import { getState, updateState } from "./state";
import { demoId, err, latency, ok } from "./util";

interface DemoChunk {
  materialId: string;
  chunkId: string;
  fileName: string;
  section: string;
  text: string;
}

function collectChunks(): DemoChunk[] {
  const docs = getState().materialDocs;
  const chunks: DemoChunk[] = [];
  for (const doc of Object.values(docs)) {
    doc.sections.forEach((section, index) => {
      chunks.push({
        materialId: doc.id,
        chunkId: `${doc.id}-c${index}`,
        fileName: doc.fileName,
        section: section.title,
        text: `${section.title}。${section.text}`,
      });
    });
  }
  return chunks;
}

function countOccurrences(haystack: string, needle: string): number {
  if (!needle) return 0;
  let count = 0;
  let offset = haystack.indexOf(needle);
  while (offset !== -1) {
    count += 1;
    offset = haystack.indexOf(needle, offset + needle.length);
  }
  return count;
}

function makeSnippet(text: string, term: string): string {
  const index = text.toLowerCase().indexOf(term.toLowerCase());
  if (index === -1) return text.slice(0, 96);
  const start = Math.max(0, index - 40);
  const end = Math.min(text.length, index + term.length + 64);
  return `${start > 0 ? "…" : ""}${text.slice(start, end).replaceAll("\n", " ")}${end < text.length ? "…" : ""}`;
}

/** 简单关键词检索：整词 + 子串匹配，按出现次数×词长打分。 */
function searchChunks(chunks: DemoChunk[], query: string, topK: number): MaterialSearchHit[] {
  const terms = query
    .toLowerCase()
    .split(/[\s,，。；;、]+/)
    .filter((term) => term.length > 0);
  if (!terms.length) return [];
  const hits: Array<MaterialSearchHit & { score: number }> = [];
  for (const chunk of chunks) {
    const haystack = chunk.text.toLowerCase();
    let score = 0;
    let matched = "";
    for (const term of terms) {
      const occurrences = countOccurrences(haystack, term);
      if (occurrences > 0) {
        score += occurrences * term.length;
        if (!matched) matched = term;
      }
    }
    if (score > 0) {
      hits.push({
        materialId: chunk.materialId,
        chunkId: chunk.chunkId,
        fileName: chunk.fileName,
        section: chunk.section,
        snippet: makeSnippet(chunk.text, matched),
        rank: score,
        score,
      });
    }
  }
  hits.sort((a, b) => b.score - a.score || a.materialId.localeCompare(b.materialId));
  return hits.slice(0, topK).map(({ score: _score, ...hit }) => hit);
}

function guessMediaType(fileName: string): string {
  if (/\.md$/i.test(fileName)) return "text/markdown";
  if (/\.(txt|log)$/i.test(fileName)) return "text/plain";
  if (/\.(png|jpe?g|webp)$/i.test(fileName)) return "image/jpeg";
  if (/\.pdf$/i.test(fileName)) return "application/pdf";
  return "application/octet-stream";
}

export function handleMaterialCommand(cmd: string, payload: Record<string, unknown> = {}): unknown {
  switch (cmd) {
    case "material_list":
      return latency(20, 80).then(() => ok(getState().materials));
    case "material_import": {
      const rawPath = String(payload.path ?? "").trim();
      if (!rawPath) return err("MATERIAL_PATH_REQUIRED", demoT().materials.pathRequired);
      const fileName = rawPath.replaceAll("\\", "/").split("/").pop() || rawPath;
      return latency(200, 600).then(() => {
        const doc: DemoMaterialDoc = {
          id: demoId("mat"),
          fileName,
          mediaType: guessMediaType(fileName),
          byteSize: 2048 + fileName.length * 16,
          status: "text_ready",
          chunkCount: 1,
          contentSha256: `demo-${demoId("sha").slice(4)}`,
          sections: [
            {
              title: demoT().materials.importSectionTitle,
              text: demoT().materials.importSectionText(fileName),
            },
          ],
        };
        const summary: MaterialSummary = {
          id: doc.id,
          fileName: doc.fileName,
          contentSha256: doc.contentSha256,
          mediaType: doc.mediaType,
          byteSize: doc.byteSize,
          status: doc.status,
          chunkCount: doc.chunkCount,
        };
        updateState((s) => {
          s.materials = [summary, ...s.materials];
          s.materialDocs[doc.id] = doc;
        });
        return ok(summary);
      });
    }
    case "material_search": {
      const query = String(payload.query ?? "").trim();
      const topKRaw = payload.topK;
      const topK = typeof topKRaw === "number" && topKRaw > 0 ? Math.min(topKRaw, 20) : 5;
      return latency(80, 260).then(() => {
        if (!query) return ok<MaterialSearchHit[]>([]);
        return ok(searchChunks(collectChunks(), query, topK));
      });
    }
    case "material_delete": {
      const id = payload.id as string;
      return latency(60, 160).then(() => {
        updateState((s) => {
          s.materials = s.materials.filter((material) => material.id !== id);
          delete s.materialDocs[id];
        });
        return ok({ ready: true });
      });
    }
    case "material_index":
      return latency(300, 800).then(() => {
        const s = getState();
        return ok({
          indexedChunks: s.materials.reduce((sum, material) => sum + material.chunkCount, 0),
          status: "ok",
        });
      });
    default:
      return undefined;
  }
}
