namespace WeChatSendGuard.App.Interop;

internal sealed class ForegroundWindowMonitor : IDisposable
{
    private readonly NativeMethods.WinEventProc _callback;
    private nint _hook;

    public ForegroundWindowMonitor()
    {
        _callback = HandleWinEvent;
    }

    public event EventHandler<nint>? ForegroundWindowChanged;

    public void Start()
    {
        if (_hook != nint.Zero)
        {
            return;
        }

        _hook = NativeMethods.SetWinEventHook(
            NativeMethods.EventSystemForeground,
            NativeMethods.EventSystemForeground,
            nint.Zero,
            _callback,
            0,
            0,
            NativeMethods.WineventOutOfContext | NativeMethods.WineventSkipOwnProcess);
        if (_hook == nint.Zero)
        {
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "Unable to subscribe to foreground window events.");
        }
    }

    public void Dispose()
    {
        if (_hook != nint.Zero)
        {
            NativeMethods.UnhookWinEvent(_hook);
            _hook = nint.Zero;
        }
    }

    private void HandleWinEvent(nint hook, uint eventType, nint hwnd, int idObject, int idChild, uint eventThread, uint eventTime)
    {
        if (eventType == NativeMethods.EventSystemForeground && hwnd != nint.Zero)
        {
            ForegroundWindowChanged?.Invoke(this, hwnd);
        }
    }
}
