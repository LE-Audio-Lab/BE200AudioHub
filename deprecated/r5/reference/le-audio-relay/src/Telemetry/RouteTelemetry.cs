namespace LEAudioRelay.Telemetry;

internal sealed class RouteTelemetry
{
    private long _capturePackets;
    private long _capturedAudioFrames;
    private long _capturedSilentFrames;

    private long _writtenFrames;
    private long _startupDroppedAudioFrames;
    private long _startupDroppedSilentFrames;
    private long _runtimeDroppedAudioFrames;
    private long _runtimeDroppedSilentFrames;

    private long _renderRequestedFrames;
    private long _renderReadFrames;
    private long _renderZeroFillFrames;
    private long _keepAliveFrames;
    private long _startupTrimFrames;

    private long _clockSilentTrimFrames;
    private long _clockGradualTrimFrames;
    private long _clockEmergencyTrimFrames;

    private long _callbackErrors;

    public void RecordCapture(
        int frames,
        bool silent,
        int writtenFrames,
        int intentionalClockTrimFrames,
        bool runtime)
    {
        Interlocked.Increment(
            ref _capturePackets);

        if (silent)
        {
            Interlocked.Add(
                ref _capturedSilentFrames,
                frames);
        }
        else
        {
            Interlocked.Add(
                ref _capturedAudioFrames,
                frames);
        }

        Interlocked.Add(
            ref _writtenFrames,
            writtenFrames);

        int dropped =
            frames -
            writtenFrames -
            intentionalClockTrimFrames;

        if (dropped <= 0)
        {
            return;
        }

        if (runtime)
        {
            if (silent)
            {
                Interlocked.Add(
                    ref _runtimeDroppedSilentFrames,
                    dropped);
            }
            else
            {
                Interlocked.Add(
                    ref _runtimeDroppedAudioFrames,
                    dropped);
            }

            return;
        }

        if (silent)
        {
            Interlocked.Add(
                ref _startupDroppedSilentFrames,
                dropped);
        }
        else
        {
            Interlocked.Add(
                ref _startupDroppedAudioFrames,
                dropped);
        }
    }

    public void RecordRender(
        int requestedFrames,
        int readFrames,
        bool keepAlive)
    {
        Interlocked.Add(
            ref _renderRequestedFrames,
            requestedFrames);

        if (keepAlive)
        {
            Interlocked.Add(
                ref _keepAliveFrames,
                requestedFrames);

            return;
        }

        Interlocked.Add(
            ref _renderReadFrames,
            readFrames);

        int zeroFill =
            requestedFrames -
            readFrames;

        if (zeroFill > 0)
        {
            Interlocked.Add(
                ref _renderZeroFillFrames,
                zeroFill);
        }
    }

    public void RecordStartupTrim(
        int frames)
    {
        if (frames > 0)
        {
            Interlocked.Add(
                ref _startupTrimFrames,
                frames);
        }
    }

    public void RecordClockSilentTrim(
        int frames)
    {
        if (frames > 0)
        {
            Interlocked.Add(
                ref _clockSilentTrimFrames,
                frames);
        }
    }

    public void RecordClockRenderTrim(
        int gradualFrames,
        int emergencyFrames)
    {
        if (gradualFrames > 0)
        {
            Interlocked.Add(
                ref _clockGradualTrimFrames,
                gradualFrames);
        }

        if (emergencyFrames > 0)
        {
            Interlocked.Add(
                ref _clockEmergencyTrimFrames,
                emergencyFrames);
        }
    }

    public void RecordCallbackError() =>
        Interlocked.Increment(
            ref _callbackErrors);

    public RouteTelemetrySnapshot Snapshot() =>
        new(
            CapturePackets:
                Interlocked.Read(
                    ref _capturePackets),

            CapturedAudioFrames:
                Interlocked.Read(
                    ref _capturedAudioFrames),

            CapturedSilentFrames:
                Interlocked.Read(
                    ref _capturedSilentFrames),

            WrittenFrames:
                Interlocked.Read(
                    ref _writtenFrames),

            StartupDroppedAudioFrames:
                Interlocked.Read(
                    ref _startupDroppedAudioFrames),

            StartupDroppedSilentFrames:
                Interlocked.Read(
                    ref _startupDroppedSilentFrames),

            RuntimeDroppedAudioFrames:
                Interlocked.Read(
                    ref _runtimeDroppedAudioFrames),

            RuntimeDroppedSilentFrames:
                Interlocked.Read(
                    ref _runtimeDroppedSilentFrames),

            RenderRequestedFrames:
                Interlocked.Read(
                    ref _renderRequestedFrames),

            RenderReadFrames:
                Interlocked.Read(
                    ref _renderReadFrames),

            RenderZeroFillFrames:
                Interlocked.Read(
                    ref _renderZeroFillFrames),

            KeepAliveFrames:
                Interlocked.Read(
                    ref _keepAliveFrames),

            StartupTrimFrames:
                Interlocked.Read(
                    ref _startupTrimFrames),

            ClockSilentTrimFrames:
                Interlocked.Read(
                    ref _clockSilentTrimFrames),

            ClockGradualTrimFrames:
                Interlocked.Read(
                    ref _clockGradualTrimFrames),

            ClockEmergencyTrimFrames:
                Interlocked.Read(
                    ref _clockEmergencyTrimFrames),

            CallbackErrors:
                Interlocked.Read(
                    ref _callbackErrors));
}

internal readonly record struct RouteTelemetrySnapshot(
    long CapturePackets,
    long CapturedAudioFrames,
    long CapturedSilentFrames,
    long WrittenFrames,
    long StartupDroppedAudioFrames,
    long StartupDroppedSilentFrames,
    long RuntimeDroppedAudioFrames,
    long RuntimeDroppedSilentFrames,
    long RenderRequestedFrames,
    long RenderReadFrames,
    long RenderZeroFillFrames,
    long KeepAliveFrames,
    long StartupTrimFrames,
    long ClockSilentTrimFrames,
    long ClockGradualTrimFrames,
    long ClockEmergencyTrimFrames,
    long CallbackErrors);
