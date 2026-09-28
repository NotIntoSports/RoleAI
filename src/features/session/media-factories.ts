import { MicStreamer, type MicStreamCallbacks, type MicStreamController } from "./mic-recorder";
import { VideoSharer, type VideoShareKind, type VideoSharerCallbacks, type VideoSharerController } from "./video-sharer";

export function defaultCreateMicStreamer(
  callbacks: MicStreamCallbacks,
  sharedContext?: AudioContext,
): MicStreamController {
  return new MicStreamer(callbacks, sharedContext);
}

export function defaultCreateVideoSharer(
  kind: VideoShareKind,
  callbacks: VideoSharerCallbacks,
): VideoSharerController {
  return new VideoSharer(kind, callbacks);
}
