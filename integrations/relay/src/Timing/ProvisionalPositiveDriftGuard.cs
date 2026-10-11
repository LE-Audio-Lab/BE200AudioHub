namespace LEAudioRelay.Timing;

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
        if (sampleRate <= 0) throw new ArgumentOutOfRangeException(nameof(sampleRate));
        if (capacityFrames <= 2) throw new ArgumentOutOfRangeException(nameof(capacityFrames));

        _targetFrames = Math.Clamp(targetFrames, 1, capacityFrames - 2);
        int oneMillisecond = Math.Max(1, sampleRate / 1000);
        int lowCandidate = _targetFrames + oneMillisecond;
        int highCandidate = _targetFrames + oneMillisecond * 2;
        int hardCandidate = _targetFrames + oneMillisecond * 10;

        _lowWaterFrames = Math.Clamp(lowCandidate, _targetFrames + 1, capacityFrames - 2);
        _highWaterFrames = Math.Clamp(highCandidate, _lowWaterFrames + 1, capacityFrames - 1);
        _hardRecenterFrames = Math.Clamp(hardCandidate, _highWaterFrames + 1, capacityFrames);
    }

    public int GetSilentFramesToSuppress(int incomingFrames, int currentFillFrames)
    {
        if (incomingFrames <= 0 || currentFillFrames <= _targetFrames) return 0;
        return Math.Min(incomingFrames, currentFillFrames - _targetFrames);
    }

    public DriftTrimResult ApplyAfterRender(SpscPcmRing ring)
    {
        int residualFill = ring.FillFrames;

        if (residualFill >= _hardRecenterFrames)
        {
            int trimmed = ring.Discard(residualFill - _targetFrames);
            _gradualTrimActive = false;
            _renderCallbackCounter = 0;
            return new DriftTrimResult(0, trimmed);
        }

        if (!_gradualTrimActive && residualFill >= _highWaterFrames)
        {
            _gradualTrimActive = true;
            _renderCallbackCounter = 0;
        }

        if (!_gradualTrimActive) return default;

        if (residualFill <= _lowWaterFrames)
        {
            _gradualTrimActive = false;
            _renderCallbackCounter = 0;
            return default;
        }

        _renderCallbackCounter++;
        if (_renderCallbackCounter < GradualTrimEveryRenderCallbacks) return default;
        _renderCallbackCounter = 0;
        return new DriftTrimResult(ring.Discard(1), 0);
    }
}

internal readonly record struct DriftTrimResult(
    int GradualFrames,
    int EmergencyFrames);
