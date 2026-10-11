namespace LEAudioRelay.Timing;

/// <summary>
/// Temporary one-sided guard for positive producer/consumer clock drift.
///
/// This is NOT clock synchronization. It controls the residual ring fill after
/// each render read around the configured target cushion. The 80 ms physical
/// ring capacity is safety headroom, not a normal latency budget.
/// </summary>
internal sealed class ProvisionalPositiveDriftGuard
{
    private const int GradualTrimEveryRenderCallbacks = 8;

    private readonly int _targetFrames;
    private readonly int _lowWaterFrames;
    private readonly int _highWaterFrames;
    private readonly int _hardRecenterFrames;

    private bool _gradualTrimActive;
    private int _renderCallbackCounter;

    public ProvisionalPositiveDriftGuard(
        int sampleRate,
        int capacityFrames,
        int targetFrames)
    {
        if (sampleRate <= 0)
        {
            throw new ArgumentOutOfRangeException(
                nameof(sampleRate));
        }

        if (capacityFrames <= 2)
        {
            throw new ArgumentOutOfRangeException(
                nameof(capacityFrames));
        }

        _targetFrames =
            Math.Clamp(
                targetFrames,
                1,
                capacityFrames - 2);

        int oneMillisecond =
            Math.Max(
                1,
                sampleRate /
                1000);

        int lowCandidate =
            _targetFrames +
            oneMillisecond;

        int highCandidate =
            _targetFrames +
            oneMillisecond *
            2;

        int hardCandidate =
            _targetFrames +
            oneMillisecond *
            10;

        _lowWaterFrames =
            Math.Clamp(
                lowCandidate,
                _targetFrames + 1,
                capacityFrames - 2);

        _highWaterFrames =
            Math.Clamp(
                highCandidate,
                _lowWaterFrames + 1,
                capacityFrames - 1);

        _hardRecenterFrames =
            Math.Clamp(
                hardCandidate,
                _highWaterFrames + 1,
                capacityFrames);
    }

    /// <summary>
    /// During source-declared silence, suppress only the already accumulated
    /// fill above the target cushion. The renderer still outputs real zeros, so
    /// silence remains the preferred inaudible correction path.
    /// </summary>
    public int GetSilentFramesToSuppress(
        int incomingFrames,
        int currentFillFrames)
    {
        if (incomingFrames <= 0 ||
            currentFillFrames <=
                _targetFrames)
        {
            return 0;
        }

        int excess =
            currentFillFrames -
            _targetFrames;

        return Math.Min(
            incomingFrames,
            excess);
    }

    /// <summary>
    /// Inspect residual fill AFTER the current render request has been read.
    /// Normal residual fill is approximately the target cushion itself.
    ///
    /// Gradual mode starts at target + 2 ms, stops at target + 1 ms, and slips
    /// one complete stereo frame every eight render callbacks. A hard recenter
    /// only occurs if residual fill reaches target + 10 ms.
    /// </summary>
    public DriftTrimResult ApplyAfterRender(
        SpscPcmRing ring)
    {
        int residualFill =
            ring.FillFrames;

        if (residualFill >=
            _hardRecenterFrames)
        {
            int requested =
                residualFill -
                _targetFrames;

            int trimmed =
                ring.Discard(
                    requested);

            _gradualTrimActive =
                false;

            _renderCallbackCounter =
                0;

            return new DriftTrimResult(
                GradualFrames: 0,
                EmergencyFrames: trimmed);
        }

        if (!_gradualTrimActive &&
            residualFill >=
                _highWaterFrames)
        {
            _gradualTrimActive =
                true;

            _renderCallbackCounter =
                0;
        }

        if (!_gradualTrimActive)
        {
            return default;
        }

        if (residualFill <=
            _lowWaterFrames)
        {
            _gradualTrimActive =
                false;

            _renderCallbackCounter =
                0;

            return default;
        }

        _renderCallbackCounter++;

        if (_renderCallbackCounter <
            GradualTrimEveryRenderCallbacks)
        {
            return default;
        }

        _renderCallbackCounter =
            0;

        int gradual =
            ring.Discard(
                1);

        return new DriftTrimResult(
            GradualFrames: gradual,
            EmergencyFrames: 0);
    }
}

internal readonly record struct DriftTrimResult(
    int GradualFrames,
    int EmergencyFrames);
