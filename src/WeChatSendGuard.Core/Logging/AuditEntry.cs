namespace WeChatSendGuard.Core.Logging;

public sealed record AuditEntry(DateTimeOffset Timestamp, Guid? ProtectedChatId, string EventType, string Result);

public interface IAuditLog
{
    ValueTask WriteAsync(AuditEntry entry, CancellationToken cancellationToken = default);
}
