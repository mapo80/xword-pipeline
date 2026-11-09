namespace Cruciverba.Xfill;

/// <summary>Return codes of the engine (as in <c>include/xfill.h</c>), plus the end of its process.</summary>
internal static class XfillCodes
{
    internal const int Ok = 0;
    internal const int ErrArg = -1;
    internal const int Cancelled = -5;

    /// <summary>The engine process ended (not a code of the C interface).</summary>
    internal const int EngineCrashed = -100;
}
