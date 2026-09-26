import { describe, expect, it, vi } from "vitest";

import { invoke } from "@tauri-apps/api/core";

import {
  activateEmbeddingConfig,
  activateModelProvider,
  activateRoleProfile,
  activateSpeechRoute,
  copyRoleProfile,
  DEFAULT_BARGE_IN,
  deleteEmbeddingConfig,
  deleteMaterial,
  deleteModelProvider,
  deleteRoleProfile,
  deleteSpeechRoute,
  discoverModelProvider,
  finalizeSessionUtterance,
  startSession,
  getConfigPublic,
  getLegacyMigrationStatus,
  importLegacySource,
  importMaterial,
  indexMaterials,
  listMaterials,
  saveEmbeddingConfig,
  saveModelProvider,
  saveRoleProfile,
  saveSpeechRoute,
  searchMaterials,
  testEmbeddingConfig,
  testModelProvider,
  testSpeechRoute,
} from "./commands";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

describe("legacy migration adapter", () => {
  it("invokes legacy_migration_status with no arguments", async () => {
    invokeMock.mockResolvedValue({
      ok: true,
      data: { applied: true, reenterSecrets: true, omitted: [] },
    });

    const result = await getLegacyMigrationStatus();

    expect(invokeMock).toHaveBeenCalledWith("legacy_migration_status");
    expect(result).toEqual({
      ok: true,
      data: { applied: true, reenterSecrets: true, omitted: [] },
    });
  });

  it("invokes legacy_import_source with the selected path", async () => {
    invokeMock.mockResolvedValue({
      ok: true,
      data: { sessions: 1, turns: 2 },
    });

    const result = await importLegacySource("E:\\\\old-install");

    expect(invokeMock).toHaveBeenCalledWith("legacy_import_source", {
      path: "E:\\\\old-install",
    });
    expect(result).toEqual({ ok: true, data: { sessions: 1, turns: 2 } });
  });
});

describe("getConfigPublic adapter", () => {
  it("invokes config_get_public with no arguments", async () => {
    invokeMock.mockResolvedValue({ ok: true, data: { configVersion: 1 } });

    const result = await getConfigPublic();

    expect(invokeMock).toHaveBeenCalledWith("config_get_public");
    expect(result).toEqual({ ok: true, data: { configVersion: 1 } });
  });

  it("surfaces a redacted public config that never carries secret values", async () => {
    // The Rust contract only ever returns SecretSlot references, never values.
    const data = {
      configVersion: 1,
      models: {
        providers: [
          { id: "p1", name: null, baseUrl: "https://one.example", credential: { reference: "providers/p1/api-key", configured: true } },
        ],
        activeProviderId: "p1",
      },
    };
    invokeMock.mockResolvedValue({ ok: true, data });

    const result = await getConfigPublic();
    const serialized = JSON.stringify(result).toLowerCase();

    expect(result).toEqual({ ok: true, data });
    expect(serialized).toContain("providers/p1/api-key");
    expect(serialized).toContain("\"configured\":true");
    for (const needle of ["must-never-cross", "password", "secretvalue", "secretcontents"]) {
      expect(serialized).not.toContain(needle);
    }
  });
});

describe("Phase 3 service adapters", () => {
  it("uses exact provider command names and payloads", async () => {
    invokeMock.mockResolvedValue({ ok: true, data: {} });
    const input = { id: "openai", name: "OpenAI", baseUrl: "https://example.test/v1", apiKey: "value" };
    await saveModelProvider(input);
    await testModelProvider("openai");
    await discoverModelProvider("openai");
    await activateModelProvider("openai");
    await deleteModelProvider("openai");
    expect(invokeMock).toHaveBeenNthCalledWith(1, "model_provider_save", { input });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "model_provider_test", { providerId: "openai" });
    expect(invokeMock).toHaveBeenNthCalledWith(3, "model_provider_discover", { providerId: "openai" });
    expect(invokeMock).toHaveBeenNthCalledWith(4, "model_provider_activate", { providerId: "openai" });
    expect(invokeMock).toHaveBeenNthCalledWith(5, "model_provider_delete", { providerId: "openai" });
  });

  it("uses exact voice route command names", async () => {
    invokeMock.mockResolvedValue({ ok: true, data: {} });
    const input = { id: "route", name: "Route", mode: "e2e" as const, asrProviderId: null, asrModelId: null, llmProviderId: null, llmModelId: null, ttsProviderId: null, ttsModelId: null, voiceId: null, e2eProviderId: "openai", e2eModelId: "realtime" };
    await saveSpeechRoute(input);
    await testSpeechRoute("route");
    await activateSpeechRoute("route");
    await deleteSpeechRoute("route");
    expect(invokeMock).toHaveBeenNthCalledWith(1, "speech_route_save", { input });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "speech_route_test", { routeId: "route" });
    expect(invokeMock).toHaveBeenNthCalledWith(3, "speech_route_activate", { routeId: "route" });
    expect(invokeMock).toHaveBeenNthCalledWith(4, "speech_route_delete", { routeId: "route" });
  });

  it("uses exact role and embedding command names", async () => {
    invokeMock.mockResolvedValue({ ok: true, data: {} });
    const role = { id: "interviewer", name: "Interviewer", systemPrompt: "Ask", openingMessage: "Hi", styleInstructions: "Short" };
    await saveRoleProfile(role);
    await copyRoleProfile({ sourceId: "interviewer", id: "copy" });
    await activateRoleProfile("interviewer");
    await deleteRoleProfile("copy");
    const embedding = { id: "primary", providerId: "openai", baseUrl: null, apiKey: null, modelId: "embed", dimensions: 8, normalized: true };
    await saveEmbeddingConfig(embedding);
    await testEmbeddingConfig("primary");
    await activateEmbeddingConfig("primary");
    await deleteEmbeddingConfig("primary");
    expect(invokeMock).toHaveBeenNthCalledWith(1, "role_profile_save", { input: role });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "role_profile_copy", { input: { sourceId: "interviewer", id: "copy" } });
    expect(invokeMock).toHaveBeenNthCalledWith(3, "role_profile_activate", { roleId: "interviewer" });
    expect(invokeMock).toHaveBeenNthCalledWith(4, "role_profile_delete", { roleId: "copy" });
    expect(invokeMock).toHaveBeenNthCalledWith(5, "embedding_config_save", { input: embedding });
    expect(invokeMock).toHaveBeenNthCalledWith(6, "embedding_config_test", { embeddingId: "primary" });
    expect(invokeMock).toHaveBeenNthCalledWith(7, "embedding_config_activate", { embeddingId: "primary" });
    expect(invokeMock).toHaveBeenNthCalledWith(8, "embedding_config_delete", { embeddingId: "primary" });
  });
});

describe("session adapters", () => {
  it("uses exact session finalize command name and payload", async () => {
    invokeMock.mockResolvedValue({ ok: true, data: {} });
    await finalizeSessionUtterance("请介绍岗位");
    expect(invokeMock).toHaveBeenCalledWith("session_finalize_utterance", { text: "请介绍岗位" });
  });

  it("starts a session without a transport selection", async () => {
    invokeMock.mockResolvedValue({ ok: true, data: {} });
    await startSession();
    await startSession({ roleProfileId: "role", voiceRouteId: "route" });
    await startSession({ roleProfileId: "role", voiceRouteId: "route", allowBargeIn: DEFAULT_BARGE_IN });
    expect(invokeMock).toHaveBeenNthCalledWith(1, "session_start", {});
    expect(invokeMock).toHaveBeenNthCalledWith(2, "session_start", { roleProfileId: "role", voiceRouteId: "route" });
    expect(invokeMock).toHaveBeenNthCalledWith(3, "session_start", { roleProfileId: "role", voiceRouteId: "route", allowBargeIn: true });
  });

  it("defaults barge-in on per the Task 1 AEC spike verdict", async () => {
    expect(DEFAULT_BARGE_IN).toBe(true);
  });
});

describe("materials adapters", () => {
  it("uses exact material command names and payloads", async () => {
    invokeMock.mockResolvedValue({ ok: true, data: {} });
    await listMaterials();
    await importMaterial("E:/docs/resume.md");
    await searchMaterials("订单服务", 5);
    await searchMaterials("订单");
    await deleteMaterial("material-1");
    await indexMaterials();
    expect(invokeMock).toHaveBeenNthCalledWith(1, "material_list");
    expect(invokeMock).toHaveBeenNthCalledWith(2, "material_import", { path: "E:/docs/resume.md" });
    expect(invokeMock).toHaveBeenNthCalledWith(3, "material_search", { query: "订单服务", topK: 5 });
    expect(invokeMock).toHaveBeenNthCalledWith(4, "material_search", { query: "订单", topK: undefined });
    expect(invokeMock).toHaveBeenNthCalledWith(5, "material_delete", { id: "material-1" });
    expect(invokeMock).toHaveBeenNthCalledWith(6, "material_index");
  });
});
