using LEAudioRelay.Telemetry;

namespace LEAudioRelay.Timing;

internal sealed class PcmRelayBoundary
{
    private const int ModeKeepAlive = 0;
    private const int ModeArmed = 1;
    private const int ModeRelay = 2;

    private readonly SpscPcmRing _ring;
    private readonly RouteTelemetry _telemetry;
    private readonly ProvisionalPositiveDriftGuard _driftGuard;

    private readonly int _blockAlign;
    private readonly int _targetCushionFrames;
    private readonly int _startupHoldFrames;

    private int _mode =
        ModeKeepAlive;

    public PcmRelayBoundary(
        int sampleRate,
        int capacityFrames,
        int blockAlign,
        int targetCushionFrames,
        int startupHoldFrames,
        RouteTelemetry telemetry)
    {
        _ring =
            new SpscPcmRing(
                capacityFrames,
                blockAlign);

        _telemetry = telemetry;

        _driftGuard =
            new ProvisionalPositiveDriftGuard(
                sampleRate,
                capacityFrames,
                targetCushionFrames);

        _blockAlign = blockAlign;
        _targetCushionFrames =
            targetCushionFrames;
        _startupHoldFrames =
            startupHoldFrames;
    }

    public int FillFrames =>
        _ring.FillFrames;

    public bool RelayActive =>
        Volatile.Read(
            ref _mode) ==
        ModeRelay;

    public void Arm() =>
        Interlocked.Exchange(
            ref _mode,
            ModeArmed);

    public void Disable() =>
        Interlocked.Exchange(
            ref _mode,
            ModeKeepAlive);

    public void Push(
        ReadOnlySpan<byte> buffer,
        bool silent)
    {
        int frames =
            buffer.Length /
            _blockAlign;

        if (frames <= 0)
        {
            return;
        }

        bool runtime =
            RelayActive;

        int intentionalClockTrim =
            0;

        int framesToWrite =
            frames;

        if (runtime &&
            silent)
        {
            intentionalClockTrim =
                _driftGuard.GetSilentFramesToSuppress(
                    frames,
                    _ring.FillFrames);

            framesToWrite -=
                intentionalClockTrim;

            _telemetry.RecordClockSilentTrim(
                intentionalClockTrim);
        }

        int maxFillFrames =
            runtime
                ? _ring.CapacityFrames
                : _startupHoldFrames;

        int written =
            framesToWrite <= 0
                ? 0
                : silent
                    ? _ring.WriteSilence(
                        framesToWrite,
                        maxFillFrames)
                    : _ring.Write(
                        buffer,
                        maxFillFrames);

        _telemetry.RecordCapture(
            frames,
            silent,
            written,
            intentionalClockTrim,
            runtime);
    }

    public int FillRenderBuffer(
        Span<byte> buffer)
    {
        buffer.Clear();

        int requestedFrames =
            buffer.Length /
            _blockAlign;

        if (requestedFrames <= 0)
        {
            return buffer.Length;
        }

        int mode =
            Volatile.Read(
                ref _mode);

        if (mode == ModeArmed)
        {
            int requiredFill =
                requestedFrames +
                _targetCushionFrames;

            int observedFill =
                _ring.FillFrames;

            if (observedFill >=
                requiredFill)
            {
                if (Interlocked.CompareExchange(
                        ref _mode,
                        ModeRelay,
                        ModeArmed) ==
                    ModeArmed)
                {
                    int excess =
                        observedFill -
                        requiredFill;

                    int trimmed =
                        _ring.Discard(
                            excess);

                    _telemetry.RecordStartupTrim(
                        trimmed);
                }

                mode =
                    Volatile.Read(
                        ref _mode);
            }
        }

        if (mode != ModeRelay)
        {
            _telemetry.RecordRender(
                requestedFrames,
                readFrames: 0,
                keepAlive: true);

            return buffer.Length;
        }

        int readFrames =
            _ring.Read(
                buffer,
                requestedFrames);

        DriftTrimResult driftTrim =
            _driftGuard.ApplyAfterRender(
                _ring);

        _telemetry.RecordClockRenderTrim(
            driftTrim.GradualFrames,
            driftTrim.EmergencyFrames);

        _telemetry.RecordRender(
            requestedFrames,
            readFrames,
            keepAlive: false);

        return buffer.Length;
    }
}
