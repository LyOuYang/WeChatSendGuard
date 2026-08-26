using System.Text.Json;
using System.Threading.Channels;
using WeChatSendGuard.Core.Logging;

namespace WeChatSendGuard.App.Services;

internal sealed class FileAuditLog : IAuditLog, IDisposable
{
    private readonly string _directory;
    private readonly Channel<AuditEntry> _channel = Channel.CreateUnbounded<AuditEntry>(new UnboundedChannelOptions
    {
        SingleReader = true,
        SingleWriter = false,
        AllowSynchronousContinuations = false,
    });
    private readonly CancellationTokenSource _stopping = new();
    private readonly Task _writer;
    private readonly JsonSerializerOptions _jsonOptions = new() { PropertyNamingPolicy = JsonNamingPolicy.CamelCase };
    private int _retentionDays;

    public FileAuditLog(string directory, int retentionDays)
    {
        _directory = directory;
        _retentionDays = Math.Clamp(retentionDays, 1, 30);
        _writer = Task.Run(WriteLoopAsync);
    }

    public void SetRetentionDays(int days) => _retentionDays = Math.Clamp(days, 1, 30);

    public ValueTask WriteAsync(AuditEntry entry, CancellationToken cancellationToken = default)
    {
        _channel.Writer.TryWrite(entry);
        return ValueTask.CompletedTask;
    }

    public void Dispose()
    {
        _channel.Writer.TryComplete();
        _stopping.Cancel();
        try
        {
            _writer.Wait(TimeSpan.FromSeconds(2));
        }
        catch (AggregateException)
        {
            // Shutdown should not prevent the application from exiting.
        }
        finally
        {
            _stopping.Dispose();
        }
    }

    private async Task WriteLoopAsync()
    {
        try
        {
            await foreach (var entry in _channel.Reader.ReadAllAsync(_stopping.Token))
            {
                try
                {
                    Directory.CreateDirectory(_directory);
                    var path = Path.Combine(_directory, $"audit-{entry.Timestamp:yyyy-MM-dd}.jsonl");
                    await File.AppendAllTextAsync(path, JsonSerializer.Serialize(entry, _jsonOptions) + Environment.NewLine, _stopping.Token);
                    RemoveExpiredFiles(entry.Timestamp);
                }
                catch (IOException)
                {
                    // Audit logging is best effort and never blocks sending.
                }
                catch (UnauthorizedAccessException)
                {
                    // Audit logging is best effort and never blocks sending.
                }
            }
        }
        catch (OperationCanceledException) when (_stopping.IsCancellationRequested)
        {
        }
    }

    private void RemoveExpiredFiles(DateTimeOffset now)
    {
        var cutoff = now.UtcDateTime.Date.AddDays(-_retentionDays);
        foreach (var file in Directory.EnumerateFiles(_directory, "audit-*.jsonl"))
        {
            try
            {
                if (File.GetLastWriteTimeUtc(file) < cutoff)
                {
                    File.Delete(file);
                }
            }
            catch (IOException)
            {
            }
            catch (UnauthorizedAccessException)
            {
            }
        }
    }
}
