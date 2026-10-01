using System.Runtime.InteropServices;
using NAudio.CoreAudioApi;

namespace AudioBridge;

internal sealed record RoleRoutingChange(string Role, bool Changed, string PreviousId);

internal sealed record CaptureMicrophoneChange(
    bool Changed,
    string CableId,
    string CableLabel,
    IReadOnlyList<RoleRoutingChange> Roles);

/// <summary>
/// 会议会话期间把 CABLE Output 设为系统默认采集端点（默认/多媒体/通信三个角色），
/// 让会议软件保持“系统默认设备”即可收到 AI 语音；会话结束按角色恢复原设备。
/// 恢复仅在当前值仍是我们设置的 CABLE 时执行，不覆盖用户中途手动修改的选择。
/// </summary>
internal static class DefaultCaptureMicrophone
{
    private static readonly Role[] AllRoles = [Role.Console, Role.Multimedia, Role.Communications];

    public static CaptureMicrophoneChange UseCableOutput(string captureId)
    {
        using var devices = new MMDeviceEnumerator();
        using var cable = devices.GetDevice(captureId);
        if (cable.State != DeviceState.Active || cable.DataFlow != DataFlow.Capture ||
            !(cable.FriendlyName.Contains("CABLE Output", StringComparison.OrdinalIgnoreCase) ||
              (cable.FriendlyName.Contains("麦克风") && cable.FriendlyName.Contains("VB-Audio"))))
            throw new InvalidOperationException("CABLE_OUTPUT_NOT_FOUND");
        var roles = new List<RoleRoutingChange>(AllRoles.Length);
        foreach (var role in AllRoles)
        {
            using var previous = devices.GetDefaultAudioEndpoint(DataFlow.Capture, role);
            roles.Add(new RoleRoutingChange(
                role.ToString(),
                !StringComparer.OrdinalIgnoreCase.Equals(previous.ID, cable.ID),
                previous.ID));
        }
        var changed = roles.Any(change => change.Changed);
        if (changed)
        {
            SwitchAllRoles(cable.ID, roles);
        }
        foreach (var role in AllRoles)
        {
            using var verified = devices.GetDefaultAudioEndpoint(DataFlow.Capture, role);
            if (!StringComparer.OrdinalIgnoreCase.Equals(verified.ID, cable.ID))
                throw new InvalidOperationException("DEFAULT_CAPTURE_MIC_VERIFY_FAILED");
        }
        return new(changed, cable.ID, cable.FriendlyName, roles);
    }

    /// <summary>按角色恢复会话前的默认采集设备。护栏：当前值仍是我们设置的
    /// <paramref name="cableId"/> 才执行，避免覆盖用户中途手动修改的选择。</summary>
    public static void Restore(
        string cableId,
        string? consoleTargetId,
        string? multimediaTargetId,
        string? communicationsTargetId)
    {
        using var devices = new MMDeviceEnumerator();
        RestoreRole(devices, Role.Console, cableId, consoleTargetId);
        RestoreRole(devices, Role.Multimedia, cableId, multimediaTargetId);
        RestoreRole(devices, Role.Communications, cableId, communicationsTargetId);
    }

    private static void SwitchAllRoles(string cableId, IReadOnlyList<RoleRoutingChange> roles)
    {
        var switched = new List<Role>(AllRoles.Length);
        try
        {
            foreach (var role in AllRoles)
            {
                SetDefault(cableId, role);
                switched.Add(role);
            }
        }
        catch
        {
            // 部分角色切换成功后失败：把已切换的角色还原，避免留下半接管状态。
            foreach (var role in switched)
            {
                var previousId = roles.First(change => change.Role == role.ToString()).PreviousId;
                try { SetDefault(previousId, role); } catch { /* 尽力还原，原异常继续上抛 */ }
            }
            throw;
        }
    }

    private static void RestoreRole(MMDeviceEnumerator devices, Role role, string expectedCurrentId, string? targetId)
    {
        string ReadCurrent()
        {
            using var current = devices.GetDefaultAudioEndpoint(DataFlow.Capture, role);
            return current.ID;
        }
        // endpointId=恢复目标（会话前设备），expectedCurrentId=护栏（CABLE）。
        RestoreIfUnchanged(role, targetId, expectedCurrentId, _ => ReadCurrent(), SetDefault);
    }

    internal static void RestoreIfUnchanged(
        Role role,
        string endpointId,
        string? expectedCurrentId,
        Func<Role, string> readCurrent,
        Action<string, Role> setDefault)
    {
        if (string.IsNullOrWhiteSpace(endpointId) || string.IsNullOrWhiteSpace(expectedCurrentId)) return;
        if (!StringComparer.OrdinalIgnoreCase.Equals(readCurrent(role), expectedCurrentId)) return;
        setDefault(endpointId, role);
        if (!StringComparer.OrdinalIgnoreCase.Equals(readCurrent(role), endpointId))
            throw new InvalidOperationException("DEFAULT_CAPTURE_MIC_RESTORE_FAILED");
    }

    private static void SetDefault(string endpointId, Role role)
    {
        var policy = (IPolicyConfig)new PolicyConfigClient();
        try
        {
            Marshal.ThrowExceptionForHR(policy.SetDefaultEndpoint(endpointId, role));
        }
        finally
        {
            Marshal.FinalReleaseComObject(policy);
        }
    }
}

[ComImport, Guid("870AF99C-171D-4F9E-AF0D-E63DF40C2BC9")]
internal class PolicyConfigClient { }

[ComImport, Guid("F8679F50-850A-41CF-9C72-430F290290C8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IPolicyConfig
{
    [PreserveSig] int GetMixFormat([MarshalAs(UnmanagedType.LPWStr)] string deviceId, IntPtr format);
    [PreserveSig] int GetDeviceFormat([MarshalAs(UnmanagedType.LPWStr)] string deviceId, int isDefault, IntPtr format);
    [PreserveSig] int ResetDeviceFormat([MarshalAs(UnmanagedType.LPWStr)] string deviceId);
    [PreserveSig] int SetDeviceFormat([MarshalAs(UnmanagedType.LPWStr)] string deviceId, IntPtr endpointFormat, IntPtr mixFormat);
    [PreserveSig] int GetProcessingPeriod([MarshalAs(UnmanagedType.LPWStr)] string deviceId, int isDefault, IntPtr defaultPeriod, IntPtr minimumPeriod);
    [PreserveSig] int SetProcessingPeriod([MarshalAs(UnmanagedType.LPWStr)] string deviceId, IntPtr period);
    [PreserveSig] int GetShareMode([MarshalAs(UnmanagedType.LPWStr)] string deviceId, IntPtr mode);
    [PreserveSig] int SetShareMode([MarshalAs(UnmanagedType.LPWStr)] string deviceId, IntPtr mode);
    [PreserveSig] int GetPropertyValue([MarshalAs(UnmanagedType.LPWStr)] string deviceId, IntPtr key, IntPtr value);
    [PreserveSig] int SetPropertyValue([MarshalAs(UnmanagedType.LPWStr)] string deviceId, IntPtr key, IntPtr value);
    [PreserveSig] int SetDefaultEndpoint([MarshalAs(UnmanagedType.LPWStr)] string deviceId, Role role);
    [PreserveSig] int SetEndpointVisibility([MarshalAs(UnmanagedType.LPWStr)] string deviceId, int visible);
}
