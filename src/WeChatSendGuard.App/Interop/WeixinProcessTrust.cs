using System.Diagnostics;

namespace WeChatSendGuard.App.Interop;

internal static class WeixinProcessTrust
{
    internal const string TrustedPath = @"C:\Program Files\Tencent\Weixin\Weixin.exe";

    public static bool IsTrusted(nint windowHandle, out int processId, out string processPath, out bool requiresElevation)
    {
        processId = 0;
        processPath = string.Empty;
        requiresElevation = false;
        if (windowHandle == nint.Zero)
        {
            return false;
        }

        NativeMethods.GetWindowThreadProcessId(windowHandle, out var rawProcessId);
        if (rawProcessId == 0)
        {
            return false;
        }

        processId = unchecked((int)rawProcessId);
        try
        {
            using var process = Process.GetProcessById(processId);
            processPath = process.MainModule?.FileName ?? string.Empty;
            return string.Equals(
                Path.GetFullPath(processPath),
                Path.GetFullPath(TrustedPath),
                StringComparison.OrdinalIgnoreCase);
        }
        catch (System.ComponentModel.Win32Exception ex) when (ex.NativeErrorCode == 5)
        {
            requiresElevation = IsWeixinProcess(processId);
            return false;
        }
        catch (Exception ex) when (ex is ArgumentException or InvalidOperationException or System.ComponentModel.Win32Exception or NotSupportedException)
        {
            return false;
        }
    }

    private static bool IsWeixinProcess(int processId)
    {
        try
        {
            using var process = Process.GetProcessById(processId);
            return string.Equals(process.ProcessName, "Weixin", StringComparison.OrdinalIgnoreCase);
        }
        catch (Exception ex) when (ex is ArgumentException or InvalidOperationException or System.ComponentModel.Win32Exception or NotSupportedException)
        {
            return false;
        }
    }
}
