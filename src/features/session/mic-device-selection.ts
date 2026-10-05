// 本机麦克风设备枚举与虚拟端点判定。选型本身由 mic-signal-probe 的
// 实测证据驱动（名称启发式在 WebView2 枚举语义差异下会失手，只作
// 探测排序与"虚拟端点"标注依据）；判定口径与后端 is_cable_output
// （src-tauri/src/prerequisites/mod.rs）对齐。
export const VIRTUAL_CAPTURE_PATTERN = /cable output|vb-audio/i;

export interface AudioInputInfo {
  deviceId: string;
  label: string;
}

/** MicStreamer 用的枚举包装：环境不支持或枚举失败一律按空处理。 */
export async function enumerateAudioInputs(): Promise<AudioInputInfo[]> {
  if (typeof navigator === "undefined" || !navigator.mediaDevices?.enumerateDevices) return [];
  try {
    const devices = await navigator.mediaDevices.enumerateDevices();
    return devices
      .filter((device) => device.kind === "audioinput")
      .map((device) => ({ deviceId: device.deviceId, label: device.label }));
  } catch {
    return [];
  }
}
