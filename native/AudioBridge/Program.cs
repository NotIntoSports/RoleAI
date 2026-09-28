using AudioBridge;
using System.Text.Json;

if (args is ["--list-output-devices"])
{
    try { Console.WriteLine(JsonSerializer.Serialize(PlaybackSession.ListOutputs())); return 0; }
    catch { Console.Error.WriteLine("OUTPUT_ENUMERATION_FAILED"); return 1; }
}

if (args is ["--list-audio-devices"])
{
    try { Console.WriteLine(JsonSerializer.Serialize(PlaybackSession.ListAudioDevices())); return 0; }
    catch { Console.Error.WriteLine("AUDIO_ENUMERATION_FAILED"); return 1; }
}

if (args is ["--play-pcm", var outputId, var rate] && int.TryParse(rate, out var sampleRate))
{
    try {
        await PlaybackSession.PlayAsync(outputId, sampleRate, Console.OpenStandardInput());
        Console.WriteLine("PLAYBACK_COMPLETED");
        return 0;
    }
    catch { Console.Error.WriteLine("PLAYBACK_FAILED"); return 1; }
}

if (args is ["--play-stream", var streamOutputId, var streamRate] && int.TryParse(streamRate, out var streamSampleRate))
{
    using var streamCancellation = new CancellationTokenSource();
    Console.CancelKeyPress += (_, eventArgs) => {
        eventArgs.Cancel = true;
        streamCancellation.Cancel();
    };
    try
    {
        await PlayStreamSession.RunAsync(streamOutputId, streamSampleRate, Console.OpenStandardInput(), streamCancellation.Token);
        Console.WriteLine(Protocol.Event("playback", new { state = "exiting" }));
        return 0;
    }
    catch (OperationCanceledException) { return 0; }
    catch (Exception error)
    {
        Console.WriteLine(Protocol.Event("playback", new {
            state = "failed",
            reason = error.GetType().Name
        }));
        Console.Error.WriteLine("PLAYBACK_FAILED");
        return 1;
    }
}

if (args is ["--monitor-input", var inputId, var monitorOutputId])
{
    using var monitorCancellation = new CancellationTokenSource();
    Console.CancelKeyPress += (_, eventArgs) => {
        eventArgs.Cancel = true;
        monitorCancellation.Cancel();
    };
    try
    {
        await MicrophoneMonitor.RunAsync(inputId, monitorOutputId, monitorCancellation.Token);
        return 0;
    }
    catch (OperationCanceledException) { return 0; }
    catch { Console.Error.WriteLine("MONITOR_FAILED"); return 1; }
}

if (args is ["--self-test"])
{
    var restored = false;
    CommunicationsMicrophone.RestoreIfUnchanged("original", "cable", () => "user-choice", _ => restored = true);
    if (restored) return 4;
    var current = "cable";
    CommunicationsMicrophone.RestoreIfUnchanged("original", "cable", () => current, value => current = value);
    if (current != "original") return 5;
    var sample = new byte[] { 0xff, 0x7f, 0x00, 0x00 };
    if (Protocol.Peak(sample) < 0.99) return 2;
    if (!Protocol.Event("ready", new { captureScope = "process-tree" }).Contains("process-tree")) return 3;
    if (!await StreamFrameSelfTestAsync()) return 6;
    if (!DrainTrackerSelfTest()) return 7;
    Console.WriteLine("AudioBridge self-test passed");
    return 0;
}

/// <summary>流式播放帧协议解析：音频帧往返、控制帧零负载、坏 kind 拒绝、EOF 边界。</summary>
static async Task<bool> StreamFrameSelfTestAsync()
{
    static async Task<StreamFrame?> ReadAsync(byte[] bytes)
    {
        using var input = new MemoryStream(bytes);
        return await new StreamFrameReader().ReadAsync(input, CancellationToken.None);
    }

    var payload = new byte[] { 0x01, 0x02, 0x03, 0x04 };
    var audio = new List<byte>(BitConverter.GetBytes((uint)StreamFrameKind.Audio));
    audio.AddRange(BitConverter.GetBytes((uint)payload.Length));
    audio.AddRange(payload);
    var read = await ReadAsync([.. audio]);
    if (read is not { Kind: StreamFrameKind.Audio } || read.Value.Payload.SequenceEqual(payload) == false) return false;

    var clear = new List<byte>(BitConverter.GetBytes((uint)StreamFrameKind.Clear));
    clear.AddRange(BitConverter.GetBytes(0u));
    var clearFrame = await ReadAsync([.. clear, .. audio]);
    if (clearFrame is not { Kind: StreamFrameKind.Clear } || clearFrame.Value.Payload.Length != 0) return false;

    var ping = new List<byte>(BitConverter.GetBytes((uint)StreamFrameKind.Ping));
    ping.AddRange(BitConverter.GetBytes(0u));
    var pingFrame = await ReadAsync([.. ping]);
    if (pingFrame is not { Kind: StreamFrameKind.Ping } || pingFrame.Value.Payload.Length != 0) return false;

    var badKind = new List<byte>(BitConverter.GetBytes(9u));
    badKind.AddRange(BitConverter.GetBytes(0u));
    try { await ReadAsync([.. badKind]); return false; }
    catch (InvalidDataException) { }

    try { await ReadAsync([.. audio.Take(audio.Count - 2)]); return false; } // 帧中途断流
    catch (EndOfStreamException) { }

    var eof = await ReadAsync([]);
    return eof is null;
}

/// <summary>DrainTracker：空置宽限、一次性回执、新音频重置计时。</summary>
static bool DrainTrackerSelfTest()
{
    var start = DateTimeOffset.UtcNow;
    var tracker = new DrainTracker(TimeSpan.FromMilliseconds(30));
    tracker.Request();
    if (tracker.Tick(start, TimeSpan.Zero)) return false;
    if (tracker.Tick(start.AddMilliseconds(20), TimeSpan.Zero)) return false;
    if (!tracker.Tick(start.AddMilliseconds(40), TimeSpan.Zero)) return false;
    if (tracker.Tick(start.AddMilliseconds(60), TimeSpan.Zero)) return false;

    tracker.Request();
    if (tracker.Tick(start, TimeSpan.FromMilliseconds(10))) return false;
    tracker.ReportAudio();
    if (tracker.Tick(start.AddMilliseconds(50), TimeSpan.Zero)) return false;
    if (!tracker.Tick(start.AddMilliseconds(80), TimeSpan.Zero)) return false;
    return true;
}

if (args is ["--set-default-communications-mic", var captureId])
{
    try
    {
        var change = CommunicationsMicrophone.UseCableOutput(captureId);
        Console.WriteLine(JsonSerializer.Serialize(new {
            changed = change.Changed,
            previousId = change.PreviousId,
            cableId = change.CableId,
            cableLabel = change.CableLabel
        }));
        return 0;
    }
    catch (Exception error)
    {
        Console.Error.WriteLine(error.Message);
        return 1;
    }
}

if (args is ["--restore-default-communications-mic", var endpointId, var expectedCurrentId])
{
    try { CommunicationsMicrophone.Restore(endpointId, expectedCurrentId); return 0; }
    catch (Exception error) { Console.Error.WriteLine(error.Message); return 1; }
}

if (args.Length != 2 || args[0] != "--pid" || !uint.TryParse(args[1], out var processId) || processId == 0)
{
    Console.Error.WriteLine(Protocol.Event("error", new {
        code = "invalid-arguments",
        message = "Usage: AudioBridge --pid <positive process id>"
    }));
    return 64;
}

using var cancellation = new CancellationTokenSource();
Console.CancelKeyPress += (_, eventArgs) => {
    eventArgs.Cancel = true;
    cancellation.Cancel();
};

try
{
    await new CaptureSession(processId).RunAsync(Console.OpenStandardOutput(), cancellation.Token);
    return 0;
}
catch (OperationCanceledException)
{
    return 0;
}
catch (Exception error)
{
    Console.Error.WriteLine(Protocol.Event("error", new {
        code = "capture-failed",
        message = error.Message
    }));
    return 1;
}
