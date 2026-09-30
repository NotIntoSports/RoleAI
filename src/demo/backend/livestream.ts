// 虚拟直播命令：livestream_generate / create_draft / get / control / insert_question
// 以及 OBS 虚拟摄像头开关。讲稿内容只从已选资料的小节拼装，不编造事实。
import type {
  LivestreamDraftInput,
  LivestreamGenerateInput,
  LivestreamRuntime,
  ObsRuntimeStatus,
} from "../../generated/bindings";

import { getState, updateState } from "./state";
import { demoId, err, latency, ok } from "./util";

import { demoT } from "./demo-text";

const OBS_CONNECTED: ObsRuntimeStatus = {
  connected: true,
  sceneReady: true,
  browserSourceReady: true,
  virtualCameraActive: false,
  errorCode: null,
};

const OBS_IDLE: ObsRuntimeStatus = {
  connected: false,
  sceneReady: false,
  browserSourceReady: false,
  virtualCameraActive: false,
  errorCode: null,
};

function emptyRuntime(title = demoT().misc.demoScriptTitle): LivestreamRuntime {
  return {
    script: {
      id: demoId("script"),
      title,
      segments: [],
      loopEnabled: false,
      confirmed: false,
      currentIndex: null,
      state: "draft",
    },
    stage: {
      productTitle: title,
      currentSubtitle: "",
      nextHint: "",
      state: "draft",
      mediaPath: null,
      mediaKind: null,
      outputState: "idle",
      outputErrorCode: null,
    },
  };
}

function runtimeFromSegments(
  title: string,
  segments: LivestreamRuntime["script"]["segments"],
  loopEnabled: boolean,
  mediaPath: string | null,
  mediaKind: LivestreamRuntime["stage"]["mediaKind"],
  confirmed: boolean,
  state: LivestreamRuntime["script"]["state"],
): LivestreamRuntime {
  const currentIndex = segments.length ? 0 : null;
  return {
    script: {
      id: demoId("script"),
      title,
      segments,
      loopEnabled,
      confirmed,
      currentIndex,
      state,
    },
    stage: {
      productTitle: title,
      currentSubtitle: currentIndex !== null ? segments[currentIndex].text : "",
      nextHint:
        segments.length > 1 && currentIndex !== null ? segments[currentIndex + 1]?.title ?? "" : "",
      state,
      mediaPath,
      mediaKind,
      outputState: state === "playing" ? "playing" : "idle",
      outputErrorCode: null,
    },
  };
}

/** 从所选资料的小节生成分段讲稿（最多 maxSegments 段）；草稿模式用传入的分段。 */
function buildSegments(
  input: LivestreamGenerateInput | LivestreamDraftInput,
  fromMaterialSections: boolean,
): LivestreamRuntime["script"]["segments"] {
  if (!fromMaterialSections) {
    const draft = input as LivestreamDraftInput;
    return draft.segments.map((segment) => ({
      id: demoId("seg"),
      title: segment.title,
      text: segment.text,
      estimatedSeconds: segment.estimatedSeconds,
      sources: segment.sources,
      status: "ready",
    }));
  }
  const generate = input as LivestreamGenerateInput;
  const docs = getState().materialDocs;
  const segments: LivestreamRuntime["script"]["segments"] = [];
  for (const materialId of generate.materialIds) {
    const doc = docs[materialId];
    if (!doc) continue;
    for (const section of doc.sections) {
      if (segments.length >= Math.max(1, generate.maxSegments)) return segments;
      segments.push({
        id: demoId("seg"),
        title: section.title,
        text: section.text,
        estimatedSeconds: Math.min(90, Math.max(15, Math.ceil(section.text.length / 5))),
        sources: [materialId],
        status: "ready",
      });
    }
  }
  return segments;
}

function persist(runtime: LivestreamRuntime): LivestreamRuntime {
  updateState((s) => {
    s.livestream = runtime;
  });
  return runtime;
}

export function handleLivestreamCommand(cmd: string, payload: Record<string, unknown> = {}): unknown {
  switch (cmd) {
    case "livestream_get":
      return latency(20, 80).then(() => ok(getState().livestream ?? emptyRuntime()));
    case "livestream_generate": {
      const input = payload.input as LivestreamGenerateInput;
      if (!input.title.trim() || !input.materialIds.length) {
        return err("LIVESTREAM_INPUT_INVALID", demoT().livestream.inputInvalid);
      }
      return latency(500, 1200).then(() => {
        const segments = buildSegments(input, true);
        if (!segments.length) return err("LIVESTREAM_NO_MATERIAL", demoT().livestream.noMaterial);
        return ok(
          persist(runtimeFromSegments(input.title, segments, input.loopEnabled, input.mediaPath, input.mediaKind, false, "ready")),
        );
      });
    }
    case "livestream_create_draft": {
      const input = payload.input as LivestreamDraftInput;
      return latency(100, 300).then(() =>
        ok(
          persist(runtimeFromSegments(input.title, buildSegments(input, false), input.loopEnabled, input.mediaPath, input.mediaKind, false, "ready")),
        ),
      );
    }
    case "livestream_control": {
      const action = payload.action as string;
      return latency(40, 140).then(() => {
        const current = getState().livestream ?? emptyRuntime();
        const segments = current.script.segments;
        let index = current.script.currentIndex;
        let state = current.script.state;
        switch (action) {
          case "confirm":
            state = "ready";
            break;
          case "start":
            if (!current.script.confirmed) return err("LIVESTREAM_NOT_CONFIRMED", demoT().livestream.notConfirmed);
            state = "playing";
            index = 0;
            break;
          case "pause":
          case "takeover":
            state = "paused";
            break;
          case "resume":
            state = "playing";
            break;
          case "previous":
            if (index !== null) index = Math.max(0, index - 1);
            break;
          case "next":
            if (index !== null && index < segments.length - 1) index += 1;
            else state = "finished";
            break;
          case "replay":
            state = "playing";
            index = 0;
            break;
          case "complete":
            state = "finished";
            break;
          default:
            return err("LIVESTREAM_ACTION_UNKNOWN", demoT().livestream.unknownAction(action));
        }
        const next = runtimeFromSegments(
          current.script.title,
          segments.map((segment, i) => ({
            ...segment,
            status: index !== null && i < index ? ("played" as const) : ("ready" as const),
          })),
          current.script.loopEnabled,
          current.stage.mediaPath,
          current.stage.mediaKind,
          action === "confirm" ? true : current.script.confirmed,
          state,
        );
        next.script.currentIndex = index;
        next.stage.currentSubtitle = index !== null ? segments[index]?.text ?? "" : "";
        next.stage.nextHint = index !== null ? segments[index + 1]?.title ?? "" : "";
        return ok(persist(next));
      });
    }
    case "livestream_insert_question": {
      const question = String(payload.question ?? "").trim();
      if (!question) return err("LIVESTREAM_QUESTION_EMPTY", demoT().livestream.questionEmpty);
      return latency(300, 800).then(() => {
        const current = getState().livestream;
        if (!current || !current.script.confirmed) {
          return err("LIVESTREAM_NOT_CONFIRMED", demoT().livestream.notConfirmed);
        }
        const answerSegment = {
          id: demoId("seg"),
          title: demoT().livestream.questionSegmentTitle(question),
          text: demoT().livestream.questionSegmentText(question),
          estimatedSeconds: 20,
          sources: [] as string[],
          status: "ready" as const,
        };
        const segments = [...current.script.segments];
        const at = current.script.currentIndex !== null ? current.script.currentIndex + 1 : segments.length;
        segments.splice(at, 0, answerSegment);
        const next = runtimeFromSegments(
          current.script.title,
          segments,
          current.script.loopEnabled,
          current.stage.mediaPath,
          current.stage.mediaKind,
          true,
          current.script.state,
        );
        next.script.currentIndex = at;
        next.stage.currentSubtitle = answerSegment.text;
        return ok(persist(next));
      });
    }
    case "obs_runtime_status":
      return latency(20, 80).then(() => ok(getState().obsConnected ? OBS_CONNECTED : OBS_IDLE));
    case "obs_virtual_camera_start":
      return latency(300, 900).then(() => {
        updateState((s) => {
          s.obsConnected = true;
        });
        return ok({ ...OBS_CONNECTED, virtualCameraActive: true });
      });
    case "obs_virtual_camera_stop":
      return latency(100, 300).then(() => {
        updateState((s) => {
          s.obsConnected = true;
        });
        return ok({ ...OBS_CONNECTED, virtualCameraActive: false });
      });
    default:
      return undefined;
  }
}
