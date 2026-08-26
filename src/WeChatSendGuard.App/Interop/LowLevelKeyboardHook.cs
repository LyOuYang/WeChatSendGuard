using System.Runtime.InteropServices;
using WeChatSendGuard.Core.Guard;

namespace WeChatSendGuard.App.Interop;

internal sealed record KeyboardStroke(uint VirtualKey, bool IsNumpadEnter, bool IsInjected, nuint ExtraInfo);

internal sealed class LowLevelKeyboardHook : IInputGate, IDisposable
{
    private readonly NativeMethods.LowLevelKeyboardProc _callback;
    private readonly nuint _injectionMarker;
    private nint _hook;
    private int _protectShortcutPressed;
    private int _suppressProtectShortcutKeyUp;
    private bool _started;

    public LowLevelKeyboardHook(nuint injectionMarker)
    {
        _injectionMarker = injectionMarker;
        _callback = HookCallback;
    }

    public event Func<KeyboardStroke, bool>? KeyDown;

    public event Func<KeyboardStroke, bool>? KeyUp;

    public event Action? ProtectCurrentRequested;

    // This predicate must be a cached-state check only. It runs on the keyboard hook thread.
    public Func<bool>? CanRequestProtectCurrent { get; set; }

    public void Start()
    {
        if (_started)
        {
            return;
        }

        _hook = NativeMethods.SetWindowsHookEx(
            NativeMethods.WhKeyboardLl,
            _callback,
            NativeMethods.GetModuleHandle(null),
            0);
        if (_hook == nint.Zero)
        {
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "Unable to install the keyboard hook.");
        }

        _started = true;
    }

    public void Stop()
    {
        if (!_started)
        {
            return;
        }

        NativeMethods.UnhookWindowsHookEx(_hook);
        _hook = nint.Zero;
        _started = false;
    }

    public void Dispose() => Stop();

    private nint HookCallback(int code, nuint wParam, nint lParam)
    {
        if (code >= 0 && (wParam == NativeMethods.WmKeyDown || wParam == NativeMethods.WmSysKeyDown || wParam == NativeMethods.WmKeyUp || wParam == NativeMethods.WmSysKeyUp))
        {
            try
            {
                var data = Marshal.PtrToStructure<NativeMethods.KbdLlHookStruct>(lParam);
                var stroke = new KeyboardStroke(
                    data.VkCode,
                    data.VkCode == NativeMethods.VkReturn && (data.Flags & NativeMethods.LlkhfExtended) != 0,
                    (data.Flags & NativeMethods.LlkhfInjected) != 0 || data.DwExtraInfo == _injectionMarker,
                    data.DwExtraInfo);

                if (stroke.VirtualKey == 0x42
                    && wParam is NativeMethods.WmKeyUp or NativeMethods.WmSysKeyUp
                    && Interlocked.Exchange(ref _suppressProtectShortcutKeyUp, 0) == 1)
                {
                    Interlocked.Exchange(ref _protectShortcutPressed, 0);
                    return 1;
                }

                if ((wParam == NativeMethods.WmKeyDown || wParam == NativeMethods.WmSysKeyDown)
                    && stroke.VirtualKey == 0x42
                    && IsKeyDown(0x11)
                    && IsKeyDown(0x12)
                    && CanRequestProtectCurrent?.Invoke() == true)
                {
                    Interlocked.Exchange(ref _suppressProtectShortcutKeyUp, 1);
                    if (Interlocked.Exchange(ref _protectShortcutPressed, 1) == 0)
                    {
                        ThreadPool.QueueUserWorkItem(_ =>
                        {
                            try
                            {
                                ProtectCurrentRequested?.Invoke();
                            }
                            catch
                            {
                                // A hotkey callback must never terminate the process.
                            }
                        });
                    }

                    return 1;
                }

                var handled = wParam is NativeMethods.WmKeyDown or NativeMethods.WmSysKeyDown
                    ? KeyDown?.Invoke(stroke) == true
                    : KeyUp?.Invoke(stroke) == true;
                if (handled)
                {
                    return 1;
                }
            }
            catch
            {
                // A failed hook callback must never block unrelated input.
            }
        }

        return NativeMethods.CallNextHookEx(_hook, code, wParam, lParam);
    }

    internal static bool IsKeyDown(int virtualKey) => (NativeMethods.GetAsyncKeyState(virtualKey) & unchecked((short)0x8000)) != 0;
}
