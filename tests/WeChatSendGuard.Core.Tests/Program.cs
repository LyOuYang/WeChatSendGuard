using WeChatSendGuard.Core.Configuration;
using WeChatSendGuard.Core.Guard;

var tests = new (string Name, Action Run)[]
{
    ("Chat title normalization", ChatTitleNormalization),
    ("Protection list matches groups and contacts", ProtectionListMatching),
    ("Exemption list confirms all other chats", ExemptionListMatching),
    ("Lists keep group and contact entries separate", ListsKeepTargetKindsSeparate),
    ("Group marker changes fail closed", GroupMarkerChangesFailClosed),
    ("Unknown group policy", UnknownGroupPolicy),
    ("Temporary bypass expiry", TemporaryBypassExpiry),
    ("Settings sanitization", SettingsSanitization),
    ("Settings store recovers from invalid JSON", SettingsStoreRecovery),
    ("Protected group export round trip", ProtectedGroupExportRoundTrip),
    ("Confirmed unchanged chat sends", ConfirmedUnchangedChatSends),
    ("Changed chat cancels send", ChangedChatCancelsSend),
    ("Unknown chat never injects", UnknownChatNeverInjects),
    ("Only one confirmation is pending", OnlyOneConfirmationIsPending),
    ("Confirmation expires", ConfirmationExpires),
};

var failures = new List<string>();
foreach (var test in tests)
{
    try
    {
        test.Run();
        Console.WriteLine($"PASS  {test.Name}");
    }
    catch (Exception exception)
    {
        failures.Add($"FAIL  {test.Name}: {exception.Message}");
    }
}

foreach (var failure in failures)
{
    Console.Error.WriteLine(failure);
}

return failures.Count == 0 ? 0 : 1;

static void ChatTitleNormalization()
{
    ExpectEqual("项目 研发 群", ChatTitleNormalizer.Normalize("  项目\t研发   群  "));
    ExpectEqual("工作群", ChatTitleNormalizer.Normalize("\r\n工作群\n"));
    ExpectEqual(string.Empty, ChatTitleNormalizer.Normalize("   "));
}

static void ProtectionListMatching()
{
    var bypasses = new TemporaryBypassRegistry();
    var group = new ProtectedChat { DisplayName = "项目研发群", MatchTitle = "项目 研发群", TargetKind = ChatTargetKind.Group };
    var contact = new ProtectedChat { DisplayName = "小王", MatchTitle = "小王", TargetKind = ChatTargetKind.Contact };
    var settings = new AppSettings { ProtectedChats = [group, contact] };

    var decision = ProtectedChatMatcher.Evaluate(GroupEditor(" 项目\t研发群 "), settings, bypasses, FixedNow());

    ExpectEqual(ProtectionDecisionKind.ConfirmProtected, decision.Kind);
    ExpectEqual(group.Id, decision.ProtectedChat?.Id);
    ExpectEqual(ProtectionDecisionKind.Pass, ProtectedChatMatcher.Evaluate(GroupEditor("普通群"), settings, bypasses, FixedNow()).Kind);
    ExpectEqual(ProtectionDecisionKind.ConfirmProtected, ProtectedChatMatcher.Evaluate(ContactEditor("小王"), settings, bypasses, FixedNow()).Kind);
    ExpectEqual(ProtectionDecisionKind.Pass, ProtectedChatMatcher.Evaluate(ContactEditor("项目研发群"), settings, bypasses, FixedNow()).Kind);
}

static void ExemptionListMatching()
{
    var group = new ProtectedChat { MatchTitle = "家庭群", TargetKind = ChatTargetKind.Group };
    var contact = new ProtectedChat { MatchTitle = "小王", TargetKind = ChatTargetKind.Contact };
    var settings = new AppSettings
    {
        RuleMode = RuleMode.ConfirmUnlessExcluded,
        ExemptedChats = [group, contact],
    };
    var bypasses = new TemporaryBypassRegistry();

    ExpectEqual(ProtectionDecisionKind.Pass, ProtectedChatMatcher.Evaluate(GroupEditor("家庭群"), settings, bypasses, FixedNow()).Kind);
    ExpectEqual(ProtectionDecisionKind.Pass, ProtectedChatMatcher.Evaluate(ContactEditor("小王"), settings, bypasses, FixedNow()).Kind);
    ExpectEqual(ProtectionDecisionKind.ConfirmUnlisted, ProtectedChatMatcher.Evaluate(GroupEditor("项目群"), settings, bypasses, FixedNow()).Kind);
    ExpectEqual(ProtectionDecisionKind.ConfirmUnlisted, ProtectedChatMatcher.Evaluate(ContactEditor("家庭群"), settings, bypasses, FixedNow()).Kind);
}

static void ListsKeepTargetKindsSeparate()
{
    var settings = SettingsValidator.Sanitize(new AppSettings
    {
        ProtectedChats =
        [
            new ProtectedChat { MatchTitle = "同名", TargetKind = ChatTargetKind.Group },
            new ProtectedChat { MatchTitle = "同名", TargetKind = ChatTargetKind.Contact },
        ],
        ExemptedChats =
        [
            new ProtectedChat { MatchTitle = "同名", TargetKind = ChatTargetKind.Group },
        ],
    });

    ExpectEqual(2, settings.ProtectedChats.Count);
    ExpectEqual(1, settings.ExemptedChats.Count);
}

static void UnknownGroupPolicy()
{
    var bypasses = new TemporaryBypassRegistry();
    ExpectEqual(
        ProtectionDecisionKind.ConfirmUnknown,
        ProtectedChatMatcher.Evaluate(GroupEditor(null), new AppSettings(), bypasses, FixedNow()).Kind);
    ExpectEqual(
        ProtectionDecisionKind.BlockUnknown,
        ProtectedChatMatcher.Evaluate(
            GroupEditor(null),
            new AppSettings { UnknownContextBehavior = UnknownContextBehavior.Block },
            bypasses,
            FixedNow()).Kind);
}

static void GroupMarkerChangesFailClosed()
{
    var chat = new ProtectedChat { MatchTitle = "工作群" };
    var context = GroupEditor("工作群") with { IsGroupChat = false };
    var decision = ProtectedChatMatcher.Evaluate(
        context,
        new AppSettings { ProtectedChats = [chat] },
        new TemporaryBypassRegistry(),
        FixedNow());
    ExpectEqual(ProtectionDecisionKind.ConfirmUnknown, decision.Kind);

    var machine = new SendGuardStateMachine();
    machine.TryBegin(context, decision, false, TimeSpan.FromSeconds(10), FixedNow(), out var pending);
    var resolution = machine.Resolve(pending!.AttemptId, ConfirmationOutcome.Confirmed, context, FixedNow().AddSeconds(1));
    ExpectFalse(resolution.ShouldInject);
}

static void TemporaryBypassExpiry()
{
    var registry = new TemporaryBypassRegistry();
    var id = Guid.NewGuid();
    registry.Grant(id, TimeSpan.FromMinutes(1), FixedNow());
    ExpectTrue(registry.IsActive(id, FixedNow().AddSeconds(30)));
    ExpectFalse(registry.IsActive(id, FixedNow().AddMinutes(2)));
    ExpectNull(registry.GetExpiry(id, FixedNow().AddMinutes(2)));
}

static void SettingsSanitization()
{
    var settings = new AppSettings
    {
        LogRetentionDays = 99,
        Confirmation = new ConfirmationSettings { HoldMilliseconds = 100, TimeoutSeconds = 99, Phrase = "   " },
        ProtectedChats =
        [
            new ProtectedChat { MatchTitle = " 工作群 " },
            new ProtectedChat { MatchTitle = "工作群" },
            new ProtectedChat { MatchTitle = string.Empty },
        ],
    };

    var result = SettingsValidator.Sanitize(settings);
    ExpectEqual(500, result.Confirmation.HoldMilliseconds);
    ExpectEqual(30, result.Confirmation.TimeoutSeconds);
    ExpectEqual("确认发送", result.Confirmation.Phrase);
    ExpectEqual(30, result.LogRetentionDays);
    ExpectEqual(1, result.ProtectedChats.Count);
    ExpectEqual("工作群", result.ProtectedChats[0].MatchTitle);
}

static void SettingsStoreRecovery()
{
    var directory = Path.Combine(Path.GetTempPath(), "WeChatSendGuard.Core.Tests", Guid.NewGuid().ToString("N"));
    var path = Path.Combine(directory, "settings.json");
    try
    {
        var store = new FileSettingsStore(path);
        store.SaveAsync(new AppSettings
        {
            ProtectedChats = [new ProtectedChat { MatchTitle = "工作群" }],
        }).GetAwaiter().GetResult();
        var loaded = store.LoadAsync().GetAwaiter().GetResult();
        ExpectEqual(1, loaded.ProtectedChats.Count);

        File.WriteAllText(path, "{invalid json");
        var recovered = store.LoadAsync().GetAwaiter().GetResult();
        ExpectEqual(0, recovered.ProtectedChats.Count);
        ExpectTrue(recovered.Enabled);
    }
    finally
    {
        if (Directory.Exists(directory))
        {
            Directory.Delete(directory, recursive: true);
        }
    }
}

static void ProtectedGroupExportRoundTrip()
{
    var original = new ProtectedChat
    {
        DisplayName = "项目研发群",
        MatchTitle = "项目研发群",
        Aliases = ["项目 研发群"],
    };
    var json = ProtectedChatExportCodec.Export([original]);
    var imported = ProtectedChatExportCodec.Import(json);
    ExpectEqual(1, imported.Count);
    ExpectEqual(original.MatchTitle, imported[0].MatchTitle);
    ExpectEqual(original.Aliases[0], imported[0].Aliases[0]);
}

static void ConfirmedUnchangedChatSends()
{
    var machine = new SendGuardStateMachine();
    var context = GroupEditor("工作群");
    ExpectTrue(machine.TryBegin(
        context,
        new ProtectionDecision(ProtectionDecisionKind.ConfirmProtected),
        false,
        TimeSpan.FromSeconds(10),
        FixedNow(),
        out var pending));

    var result = machine.Resolve(pending!.AttemptId, ConfirmationOutcome.Confirmed, context, FixedNow().AddSeconds(1));
    ExpectTrue(result.ShouldInject);
    ExpectNull(machine.Current);
}

static void ChangedChatCancelsSend()
{
    var machine = new SendGuardStateMachine();
    var context = GroupEditor("工作群");
    machine.TryBegin(context, new ProtectionDecision(ProtectionDecisionKind.ConfirmProtected), false, TimeSpan.FromSeconds(10), FixedNow(), out var pending);

    var result = machine.Resolve(pending!.AttemptId, ConfirmationOutcome.Confirmed, GroupEditor("另一个群"), FixedNow().AddSeconds(1));
    ExpectFalse(result.ShouldInject);
}

static void UnknownChatNeverInjects()
{
    var machine = new SendGuardStateMachine();
    var context = GroupEditor(null);
    machine.TryBegin(context, new ProtectionDecision(ProtectionDecisionKind.ConfirmUnknown), false, TimeSpan.FromSeconds(10), FixedNow(), out var pending);

    var result = machine.Resolve(pending!.AttemptId, ConfirmationOutcome.Confirmed, context, FixedNow().AddSeconds(1));
    ExpectFalse(result.ShouldInject);
}

static void OnlyOneConfirmationIsPending()
{
    var machine = new SendGuardStateMachine();
    var decision = new ProtectionDecision(ProtectionDecisionKind.ConfirmProtected);
    ExpectTrue(machine.TryBegin(GroupEditor("工作群"), decision, false, TimeSpan.FromSeconds(10), FixedNow(), out _));
    ExpectFalse(machine.TryBegin(GroupEditor("工作群"), decision, false, TimeSpan.FromSeconds(10), FixedNow(), out _));
}

static void ConfirmationExpires()
{
    var machine = new SendGuardStateMachine();
    var context = GroupEditor("工作群");
    machine.TryBegin(context, new ProtectionDecision(ProtectionDecisionKind.ConfirmProtected), false, TimeSpan.FromSeconds(5), FixedNow(), out var pending);

    var result = machine.Resolve(pending!.AttemptId, ConfirmationOutcome.Confirmed, context, FixedNow().AddSeconds(6));
    ExpectFalse(result.ShouldInject);
}

static ChatContext GroupEditor(string? title) => new()
{
    WindowHandle = 42,
    ProcessId = 7,
    IsTrustedWeixin = true,
    IsCompatibilityAvailable = true,
    IsMessageEditorFocused = true,
    IsGroupChat = true,
    ChatTitle = title,
    Generation = 1,
};

static ChatContext ContactEditor(string? title) => GroupEditor(title) with
{
    IsGroupChat = false,
    IsContactChat = true,
};

static DateTimeOffset FixedNow() => new(2026, 8, 26, 8, 0, 0, TimeSpan.Zero);

static void ExpectTrue(bool value)
{
    if (!value)
    {
        throw new InvalidOperationException("Expected true.");
    }
}

static void ExpectFalse(bool value)
{
    if (value)
    {
        throw new InvalidOperationException("Expected false.");
    }
}

static void ExpectNull(object? value)
{
    if (value is not null)
    {
        throw new InvalidOperationException("Expected null.");
    }
}

static void ExpectEqual<T>(T expected, T actual)
{
    if (!EqualityComparer<T>.Default.Equals(expected, actual))
    {
        throw new InvalidOperationException($"Expected '{expected}', received '{actual}'.");
    }
}
