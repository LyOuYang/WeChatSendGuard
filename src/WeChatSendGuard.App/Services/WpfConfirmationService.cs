using System.Windows.Threading;
using System.Windows.Interop;
using WeChatSendGuard.App.Windows;
using WeChatSendGuard.Core.Configuration;
using WeChatSendGuard.Core.Guard;

namespace WeChatSendGuard.App.Services;

internal sealed class WpfConfirmationService(Dispatcher dispatcher) : IConfirmationService
{
    private readonly object _sync = new();
    private ConfirmationWindow? _activeWindow;

    public Task<ConfirmationOutcome> ConfirmAsync(
        PendingConfirmation pending,
        ConfirmationSettings settings,
        CancellationToken cancellationToken = default)
    {
        var completion = new TaskCompletionSource<ConfirmationOutcome>(TaskCreationOptions.RunContinuationsAsynchronously);
        _ = dispatcher.BeginInvoke(() => ShowWindow(pending, settings, completion, cancellationToken));
        return completion.Task;
    }

    public void CancelActive()
    {
        _ = dispatcher.BeginInvoke(() =>
        {
            lock (_sync)
            {
                _activeWindow?.Cancel();
            }
        });
    }

    public bool OwnsWindow(nint windowHandle)
    {
        if (windowHandle == nint.Zero)
        {
            return false;
        }

        lock (_sync)
        {
            return _activeWindow is not null
                && new WindowInteropHelper(_activeWindow).Handle == windowHandle;
        }
    }

    private void ShowWindow(
        PendingConfirmation pending,
        ConfirmationSettings settings,
        TaskCompletionSource<ConfirmationOutcome> completion,
        CancellationToken cancellationToken)
    {
        lock (_sync)
        {
            if (cancellationToken.IsCancellationRequested)
            {
                completion.TrySetResult(ConfirmationOutcome.Cancelled);
                return;
            }

            if (_activeWindow is not null)
            {
                completion.TrySetResult(ConfirmationOutcome.Cancelled);
                return;
            }

            var window = new ConfirmationWindow(pending, settings);
            _activeWindow = window;
            window.Completion.ContinueWith(
                task =>
                {
                    completion.TrySetResult(task.GetAwaiter().GetResult());
                    _ = dispatcher.BeginInvoke(() =>
                    {
                        lock (_sync)
                        {
                            if (ReferenceEquals(_activeWindow, window))
                            {
                                _activeWindow = null;
                            }
                        }
                    });
                },
                CancellationToken.None,
                TaskContinuationOptions.ExecuteSynchronously,
                TaskScheduler.Default);
            window.Show();
            window.Activate();
            if (cancellationToken.CanBeCanceled)
            {
                cancellationToken.Register(() => _ = dispatcher.BeginInvoke(window.Cancel));
            }
        }
    }
}
