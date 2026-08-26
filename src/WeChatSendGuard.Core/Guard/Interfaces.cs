using WeChatSendGuard.Core.Configuration;

namespace WeChatSendGuard.Core.Guard;

public interface IChatContextProvider
{
    ChatContext Current { get; }

    ChatContext RefreshNow();
}

public interface IInputGate
{
    void Start();

    void Stop();
}

public interface IInputInjector
{
    void SendEnter(bool isNumpadEnter);
}

public interface IConfirmationService
{
    Task<ConfirmationOutcome> ConfirmAsync(
        PendingConfirmation pending,
        ConfirmationSettings settings,
        CancellationToken cancellationToken = default);

    void CancelActive();

    bool OwnsWindow(nint windowHandle);
}
