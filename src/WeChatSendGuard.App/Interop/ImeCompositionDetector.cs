namespace WeChatSendGuard.App.Interop;

internal static class ImeCompositionDetector
{
    public static bool IsComposing(nint windowHandle)
    {
        var inputContext = NativeMethods.ImmGetContext(windowHandle);
        if (inputContext == nint.Zero)
        {
            return false;
        }

        try
        {
            return NativeMethods.ImmGetCompositionStringW(inputContext, NativeMethods.GcsCompStr, nint.Zero, 0) > 0;
        }
        finally
        {
            NativeMethods.ImmReleaseContext(windowHandle, inputContext);
        }
    }
}
