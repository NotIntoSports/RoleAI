# AudioBridge

Windows 本地会议进程音频采集与指定渲染设备播放。复用 MIT 许可的
NAudio.Wasapi `3.0.0-preview.20`，.NET 10 SDK；发布产物自带运行时，用户无需安装 SDK。

开发/打包前执行：

```powershell
dotnet publish native/AudioBridge/AudioBridge.csproj -c Release -o native/AudioBridge/publish
native/AudioBridge/publish/AudioBridge.exe --self-test
```

Tauri 将发布的 EXE 放入 `audio-bridge/AudioBridge.exe`。不得提交约 100 MB 的
发布产物；构建机器必须先发布该组件。SDK 仅是构建依赖。

- `--pid <PID>`：既有进程树回环采集。应用启动前校验会议进程白名单。
- `--list-output-devices`：只枚举活动渲染设备，输出 ID/name JSON，不改变默认设备。
- `--play-pcm <endpoint ID> <sample rate>`：从 stdin 读取 16-bit 单声道 PCM；
  最多 16 MiB，8–48 kHz，只输出到指定设备。不选择系统默认设备，不落盘。
  完成后退出 0；失败退出非零。调用方取消/超时会终止本次播放进程。
- `--play-stream <endpoint ID> <sample rate>`：常驻流式播放（实时语音会话用）。
  stdin 帧协议 `[u32 kind LE][u32 len LE][payload]`，kind：1=音频（16-bit 单声道 PCM）、
  2=clear（清空缓冲立即静音，用于打断）、3=drain（缓冲播净后回执）。stdout 输出
  `playback` 事件行：`started`（设备已打开，会话开始即发）、`cleared`、`drained`、
  `overflow`（在途缓冲超 60s 被清）。设备在启动时即打开并持续输出静音，把 WASAPI
  激活成本移出首响延迟；缓冲预热 120ms 后开始出声。stdin EOF 把残余播净后退出 0。
- `--monitor-input <capture endpoint ID> <render endpoint ID>`：人工接管期间把已保存的物理麦克风送入虚拟声卡渲染端；缓冲上限两秒，停止/异常时由主程序终止并恢复原线路。
- `--set-default-capture-mic <capture endpoint ID>`：会议会话期间把 CABLE Output 设为系统默认采集端点（默认/多媒体/通信三个角色），stdout 输出各角色会话前设备 JSON；任一角色切换失败时自动还原已切换的角色。
- `--restore-default-capture-mic <cableId> <consoleTargetId> <multimediaTargetId> <communicationsTargetId>`：按角色恢复会话前设备；护栏——当前值仍是 CABLE 才恢复，空目标（该角色未变更）跳过，不覆盖用户中途手改的选择。

工作台默认仅文字。会议会话启动时自动接管系统默认麦克风（三个角色）并在结束时还原，
跟随“系统默认设备”的会议软件（腾讯会议、Zoom、飞书、钉钉等默认行为）无需手动选择；
在会议软件里显式选择过设备的应用仍需首次手动选一次 CABLE Output。AI 语音始终播放到
虚拟声卡渲染端（如 CABLE Input）。只有扬声器时不能据此声称声音已进入会议。实机播放/会议测试须显式启用；自动化测试不播放候选人数据。

限制：自动分句、回声抑制与场景触发已有自动化覆盖，但当前主机没有虚拟声卡，仍不能据此声称会议对端已实际听到 AI 或人工麦克风。
