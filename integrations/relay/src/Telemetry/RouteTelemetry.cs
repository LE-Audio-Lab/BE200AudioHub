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
        int frames, bool silent, int writtenFrames,
        int intentionalClockTrimFrames, bool runtime)
    {
        Interlocked.Increment(ref _capturePackets);
        Interlocked.Add(
            ref (silent ? ref _capturedSilentFrames : ref _capturedAudioFrames),
            frames);
        Interlocked.Add(ref _writtenFrames, writtenFrames);

        int dropped = frames - writtenFrames - intentionalClockTrimFrames;
        if (dropped <= 0) return;

        if (runtime)
        {
            Interlocked.Add(
                ref (silent ? ref _runtimeDroppedSilentFrames : ref _runtimeDroppedAudioFrames),
                dropped);
        }
        else
        {
            Interlocked.Add(
                ref (silent ? ref _startupDroppedSilentFrames : ref _startupDroppedAudioFrames),
                dropped);
        }
    }

    public void RecordRender(int requestedFrames, int readFrames, bool keepAlive)
    {
        Interlocked.Add(ref _renderRequestedFrames, requestedFrames);
        if (keepAlive)
        {
            Interlocked.Add(ref _keepAliveFrames, requestedFrames);
            return;
        }
        Interlocked.Add(ref _renderReadFrames, readFrames);
        int zeroFill = requestedFrames - readFrames;
        if (zeroFill > 0) Interlocked.Add(ref _renderZeroFillFrames, zeroFill);
    }

    public void RecordStartupTrim(int frames)
    {
        if (frames > 0) Interlocked.Add(ref _startupTrimFrames, frames);
    }

    public void RecordClockSilentTrim(int frames)
    {
        if (frames > 0) Interlocked.Add(ref _clockSilentTrimFrames, frames);
    }

    public void RecordClockRenderTrim(int gradualFrames, int emergencyFrames)
    {
        if (gradualFrames > 0) Interlocked.Add(ref _clockGradualTrimFrames, gradualFrames);
        if (emergencyFrames > 0) Interlocked.Add(ref _clockEmergencyTrimFrames, emergencyFrames);
    }

    public void RecordCallbackError() => Interlocked.Increment(ref _callbackErrors);
}
