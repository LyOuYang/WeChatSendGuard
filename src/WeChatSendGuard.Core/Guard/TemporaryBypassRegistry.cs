using System.Collections.Concurrent;

namespace WeChatSendGuard.Core.Guard;

public sealed class TemporaryBypassRegistry
{
    private readonly ConcurrentDictionary<Guid, DateTimeOffset> _entries = new();

    public void Grant(Guid protectedChatId, TimeSpan duration, DateTimeOffset now)
    {
        ArgumentOutOfRangeException.ThrowIfLessThanOrEqual(duration, TimeSpan.Zero);
        _entries[protectedChatId] = now.Add(duration);
    }

    public bool IsActive(Guid protectedChatId, DateTimeOffset now)
    {
        if (!_entries.TryGetValue(protectedChatId, out var expiresAt))
        {
            return false;
        }

        if (expiresAt > now)
        {
            return true;
        }

        _entries.TryRemove(protectedChatId, out _);
        return false;
    }

    public DateTimeOffset? GetExpiry(Guid protectedChatId, DateTimeOffset now)
    {
        return IsActive(protectedChatId, now) && _entries.TryGetValue(protectedChatId, out var expiry)
            ? expiry
            : null;
    }

    public void Clear(Guid protectedChatId)
    {
        _entries.TryRemove(protectedChatId, out _);
    }
}
