using WeChatSendGuard.Core.Guard;

namespace WeChatSendGuard.App.Interop;

internal sealed class InputInjector(nuint marker) : IInputInjector
{
    private static readonly int NativeInputSize = Marshal.SizeOf<NativeMethods.Input>();
    private static readonly int ExpectedNativeInputSize = IntPtr.Size == 8 ? 40 : 28;

    public void SendEnter(bool isNumpadEnter)
    {
        if (NativeInputSize != ExpectedNativeInputSize)
        {
            throw new InputInjectionException(
                "input-layout-invalid",
                NativeInputSize,
                sentInputCount: 0,
                win32Error: 0);
        }

        var virtualKey = (ushort)NativeMethods.VkReturn;
        var inputs = new[]
        {
            new NativeMethods.Input
            {
                Type = NativeMethods.InputKeyboard,
                Union = new NativeMethods.InputUnion
                {
                    Keyboard = new NativeMethods.KeyboardInput
                    {
                        Vk = virtualKey,
                        Flags = isNumpadEnter ? NativeMethods.KeyeventfExtendedKey : 0,
                        ExtraInfo = marker,
                    },
                },
            },
            new NativeMethods.Input
            {
                Type = NativeMethods.InputKeyboard,
                Union = new NativeMethods.InputUnion
                {
                    Keyboard = new NativeMethods.KeyboardInput
                    {
                        Vk = virtualKey,
                        Flags = NativeMethods.KeyeventfKeyup | (isNumpadEnter ? NativeMethods.KeyeventfExtendedKey : 0),
                        ExtraInfo = marker,
                    },
                },
            },
        };

        var sent = NativeMethods.SendInput((uint)inputs.Length, inputs, NativeInputSize);
        if (sent != inputs.Length)
        {
            throw new InputInjectionException(
                "send-input-failed",
                NativeInputSize,
                sent,
                Marshal.GetLastWin32Error());
        }
    }
}

internal sealed class InputInjectionException(
    string stage,
    int inputSize,
    uint sentInputCount,
    int win32Error) : Exception(stage)
{
    public string Stage { get; } = stage;

    public int InputSize { get; } = inputSize;

    public uint SentInputCount { get; } = sentInputCount;

    public int Win32Error { get; } = win32Error;
}
