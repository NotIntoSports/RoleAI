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

工作台默认仅文字。会议语音应选择虚拟声卡渲染端（如 CABLE Input），并在会议软件中
选择配对采集端（如 CABLE Output）作为麦克风。不会自动修改全系统默认设备。
只有扬声器时不能据此声称声音已进入会议。实机播放/会议测试须显式启用；自动化测试不播放候选人数据。

限制：自动分句、回声抑制与场景触发已有自动化覆盖，但当前主机没有虚拟声卡，仍不能据此声称会议对端已实际听到 AI 或人工麦克风。
