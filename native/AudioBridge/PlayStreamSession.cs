using NAudio.CoreAudioApi;
using NAudio.Wave;

namespace AudioBridge;

/// <summary>stdin 帧头：[u32 kind LE][u32 len LE][payload]。kind 1=audio 2=clear 3=drain 4=ping。</summary>
internal enum StreamFrameKind : uint
{
    Audio = 1,
    Clear = 2,
    Drain = 3,
    Ping = 4,
}

internal readonly record struct StreamFrame(StreamFrameKind Kind, byte[] Payload);

/// <summary>
/// 播净回执状态机。BufferedWaveProvider 清空只表示样本已进入 WASAPI 队列，
/// 不代表扬声器已经结束；继续空置一个设备缓冲宽限期后才允许上报 drained。
/// </summary>
internal sealed class DrainTracker(TimeSpan? grace = null)
{
    public static readonly TimeSpan DefaultGrace = TimeSpan.FromMilliseconds(600);

    private readonly TimeSpan grace = grace ?? DefaultGrace;
    private bool pending;
    private DateTimeOffset? emptySince;

    public bool Pending => pending;

    public void Request()
    {
        pending = true;
        emptySince = null;
    }

    public void ReportAudio()
    {
        if (pending) emptySince = null;
    }

    public void Reset()
    {
        pending = false;
        emptySince = null;
    }

    public bool Tick(DateTimeOffset now, TimeSpan bufferedDuration)
    {
        if (!pending) return false;
        if (bufferedDuration > TimeSpan.Zero)
        {
            emptySince = null;
            return false;
        }

        emptySince ??= now;
        if (now - emptySince.Value < grace) return false;

        Reset();
        return true;
    }
}

/// <summary>
/// 从流中逐帧解析；单帧 payload 上限 1 MiB（100ms@48k ≈ 96KB，留足余量），
/// 超限/控制帧带负载/未知 kind 视为协议错。纯逻辑，self-test 用内存流直接验证。
/// </summary>
internal sealed class StreamFrameReader
{
    public const int MaxPayloadBytes = 1024 * 1024;
    private readonly byte[] header = new byte[8];

    /// <summary>读到完整帧返回帧；流在帧边界干净结束返回 null；帧中途断流抛 EndOfStreamException。</summary>
    public async Task<StreamFrame?> ReadAsync(Stream input, CancellationToken cancellation)
    {
        if (!await TryFillAsync(input, header, cancellation)) return null;
        var kindValue = BitConverter.ToUInt32(header, 0);
        var length = BitConverter.ToUInt32(header, 4);
        if (kindValue is not ((uint)StreamFrameKind.Audio or (uint)StreamFrameKind.Clear or (uint)StreamFrameKind.Drain or (uint)StreamFrameKind.Ping))
            throw new InvalidDataException("INVALID_FRAME_KIND");
        if (length > MaxPayloadBytes) throw new InvalidDataException("FRAME_TOO_LARGE");
        if (kindValue != (uint)StreamFrameKind.Audio && length != 0) throw new InvalidDataException("CONTROL_FRAME_WITH_PAYLOAD");
        var payload = new byte[length];
        if (length > 0 && !await TryFillAsync(input, payload, cancellation))
            throw new EndOfStreamException();
        return new StreamFrame((StreamFrameKind)kindValue, payload);
    }

    /// <summary>填满 target；在起点干净结束返回 false，读了一半断流抛 EndOfStreamException。</summary>
    private static async Task<bool> TryFillAsync(Stream input, byte[] target, CancellationToken cancellation)
    {
        var filled = 0;
        while (filled < target.Length)
        {
            var count = await input.ReadAsync(target.AsMemory(filled), cancellation);
            if (count == 0)
            {
                if (filled == 0) return false;
                throw new EndOfStreamException();
            }
            filled += count;
        }
        return true;
    }
}

/// <summary>
/// 常驻流式播放。结构：读帧任务把音频写入 BufferedWaveProvider（内部加锁，跨线程安全），
/// 监督循环每 20ms 检查 drain 回执与故障；设备在会话开始即打开并持续输出静音，
/// WASAPI 激活成本（数百毫秒）移出首响延迟路径。stdin EOF 把残余播净后退出。
/// </summary>
internal static class PlayStreamSession
{
    public const int MaxBufferedSeconds = 60;
    private const int PollMilliseconds = 20;

    public static async Task RunAsync(string endpointId, int sampleRate, Stream input, CancellationToken cancellation)
    {
        if (sampleRate is < 8000 or > 48000 || string.IsNullOrWhiteSpace(endpointId))
            throw new ArgumentException("INVALID_PLAYBACK_FORMAT");

        using var devices = new MMDeviceEnumerator();
        using var device = devices.GetDevice(endpointId);
        if (device.State != DeviceState.Active || device.DataFlow != DataFlow.Render)
            throw new InvalidOperationException("OUTPUT_DEVICE_UNAVAILABLE");

        var format = new WaveFormat(sampleRate, 16, 1);
        var buffer = new BufferedWaveProvider(format)
        {
            DiscardOnBufferOverflow = false,
            ReadFully = true, // underrun 补零：空闲与打断清空后输出静音，播放器无需重启
        };
          await using var player = await new WasapiPlayerBuilder()
              .WithDevice(device)
              .WithSharedMode()
              .WithEventSync()
              // Realtime delta 到达存在几十到五百毫秒级抖动，实测最大间隙
              // 可达 2s+。较小设备缓冲会在短暂无数据时插入静音，听感为
              // 句中掉字；800ms 吸收常见抖动（dependency-decisions 记录
              // 50→250→500→800 的演进），仍在可感延迟预算内。
              .WithLatency(800)
              .BuildAsync();
        var playerFailure = new TaskCompletionSource<Exception?>(TaskCreationOptions.RunContinuationsAsynchronously);
        player.PlaybackStopped += (_, stopped) =>
        {
            if (stopped.Exception is not null) playerFailure.TrySetResult(stopped.Exception);
        };
        player.Init(buffer);
        player.Play();
        Console.WriteLine(Protocol.Event("playback", new { state = "started" }));

        var drainTracker = new DrainTracker();
        var readerFault = new TaskCompletionSource<Exception?>(TaskCreationOptions.RunContinuationsAsynchronously);
        var inputClosed = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        _ = Task.Run(async () =>
        {
            try
            {
                var frames = new StreamFrameReader();
                while (true)
                {
                    StreamFrame? frame;
                    try
                    {
                        frame = await frames.ReadAsync(input, cancellation);
                    }
                    catch (EndOfStreamException)
                    {
                        return;
                    }
                    if (frame is null) return; // stdin EOF：进入播净收尾
                    switch (frame.Value.Kind)
                    {
                        case StreamFrameKind.Audio:
                            if (frame.Value.Payload.Length > 0)
                            {
                                if (buffer.BufferedDuration > TimeSpan.FromSeconds(MaxBufferedSeconds))
                                {
                                    buffer.ClearBuffer();
                                    Console.WriteLine(Protocol.Event("playback", new { state = "overflow" }));
                                }
                                buffer.AddSamples(frame.Value.Payload, 0, frame.Value.Payload.Length);
                                drainTracker.ReportAudio();
                            }
                            break;
                        case StreamFrameKind.Clear:
                            buffer.ClearBuffer();
                            drainTracker.Reset();
                            Console.WriteLine(Protocol.Event("playback", new { state = "cleared" }));
                            break;
                        case StreamFrameKind.Drain:
                            // drained 语义是“设备已播净”，不是“provider 已清空”。
                            drainTracker.Request();
                            break;
                        case StreamFrameKind.Ping:
                            // 保活/健康探测帧：无副作用。
                            break;
                    }
                }
            }
            catch (Exception error)
            {
                readerFault.TrySetResult(error);
            }
            finally
            {
                inputClosed.TrySetResult();
            }
        }, cancellation);

        try
        {
            while (true)
            {
                if (readerFault.Task.IsCompleted)
                    throw readerFault.Task.Result ?? new InvalidOperationException("PLAYBACK_FAILED");
                if (playerFailure.Task.IsCompleted)
                    throw playerFailure.Task.Result ?? new InvalidOperationException("PLAYBACK_FAILED");
                if (drainTracker.Tick(DateTimeOffset.UtcNow, buffer.BufferedDuration))
                    Console.WriteLine(Protocol.Event("playback", new { state = "drained" }));
                if (inputClosed.Task.IsCompleted) break;
                await Task.Delay(PollMilliseconds, cancellation);
            }

            // stdin 已关闭：把残余缓冲播净再退出，避免截尾。
            while (buffer.BufferedDuration > TimeSpan.Zero && !cancellation.IsCancellationRequested)
            {
                if (readerFault.Task.IsCompleted)
                    throw readerFault.Task.Result ?? new InvalidOperationException("PLAYBACK_FAILED");
                if (playerFailure.Task.IsCompleted)
                    throw playerFailure.Task.Result ?? new InvalidOperationException("PLAYBACK_FAILED");
                await Task.Delay(PollMilliseconds, cancellation);
            }
        }
        finally
        {
            player.Stop();
        }
        if (readerFault.Task.IsCompleted)
            throw readerFault.Task.Result ?? new InvalidOperationException("PLAYBACK_FAILED");
        if (playerFailure.Task.IsCompleted)
            throw playerFailure.Task.Result ?? new InvalidOperationException("PLAYBACK_FAILED");
        cancellation.ThrowIfCancellationRequested();
    }
}
