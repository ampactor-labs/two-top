#!/usr/bin/env python3
"""Generate 2-Top's synthesized sound effects + ambient bed (Phase 18 Task 5.3).

Bone Cathedral in sound: dry, percussive, bone-and-blood. Every cue is
synthesized from first principles with the Python standard library only
(`wave`, `math`, `struct`, a seeded `random.Random`) so the audio is
*deterministic* — regenerating on any machine yields byte-identical WAVs,
the same discipline the art generator follows. No samples, no external DSP
libraries, no network.

Format: 22050 Hz, mono, 16-bit PCM. One-shot SFX are peak-normalized to
-3 dBFS so they punch; the ambient loop sits back at a -18 dBFS bed so it
never competes with gameplay cues. The ambient loop is built seamless by
construction (whole-cycle detuned drones + an end-zeroed noise swell), so
it wraps without a click.

Cue → game event wiring lives in `crates/render` (`GameAudioPlugin`), fired
off the same render-side sim-event edges the effect sprites use. The GO toll
is not a separate file: it is `countdown_toll.wav` played back at speed 1.25.

Run: `python3 scripts/generate_audio.py`  → writes assets/audio/*.wav
"""

from __future__ import annotations

import math
import os
import random
import struct
import wave

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
OUT_DIR = os.path.join(ROOT, "assets", "audio")

SR = 22050  # sample rate (Hz) — matches the chunky, dry aesthetic; small files.

# Seed = ASCII bytes of "2TOP" (0x32='2' 0x54='T' 0x4F='O' 0x50='P'). A fixed
# seed makes the noise beds reproducible: same residue every regeneration.
SEED = 0x32544F50

# Target peak levels (linear amplitude from dBFS).
SFX_PEAK = 10.0 ** (-3.0 / 20.0)   # ~0.7079
AMBIENT_PEAK = 10.0 ** (-18.0 / 20.0)  # ~0.1259


# ---------------------------------------------------------------------------
# Tiny DSP toolkit (stdlib only). A "signal" is a list[float] in [-1, 1]-ish;
# normalization at write time guarantees the final clamp.
# ---------------------------------------------------------------------------


def n_samples(seconds: float) -> int:
    return int(round(SR * seconds))


def silence(seconds: float) -> list[float]:
    return [0.0] * n_samples(seconds)


def sine(freq: float, seconds: float, phase: float = 0.0) -> list[float]:
    w = 2.0 * math.pi * freq
    return [math.sin(w * (i / SR) + phase) for i in range(n_samples(seconds))]


def saw(freq: float, seconds: float) -> list[float]:
    """Naive (aliased) bipolar sawtooth. Aliasing is fine — it adds the gritty
    high-end the gore-revival pole wants; nothing here is mastered hi-fi."""
    out = []
    for i in range(n_samples(seconds)):
        t = i / SR
        ph = freq * t
        out.append(2.0 * (ph - math.floor(ph + 0.5)))
    return out


def square(freq: float, seconds: float) -> list[float]:
    return [1.0 if s >= 0.0 else -1.0 for s in sine(freq, seconds)]


def noise(seconds: float, rng: random.Random) -> list[float]:
    return [rng.uniform(-1.0, 1.0) for _ in range(n_samples(seconds))]


def exp_env(seconds: float, tau: float) -> list[float]:
    """Exponential decay envelope, env(t) = e^(-t/tau)."""
    return [math.exp(-(i / SR) / tau) for i in range(n_samples(seconds))]


def attack(sig: list[float], ms: float) -> list[float]:
    """Linear fade-in over the first `ms` to kill the onset click. Sharp,
    percussive cues use a tiny value (~0.3 ms); tonal cues a few ms."""
    a = max(1, n_samples(ms / 1000.0))
    out = list(sig)
    for i in range(min(a, len(out))):
        out[i] *= i / a
    return out


def one_pole_lp(sig: list[float], cutoff) -> list[float]:
    """One-pole low-pass. `cutoff` is either a constant Hz or a per-sample
    list[float] of Hz (for sweeps). a = 1 - e^(-2*pi*fc/SR)."""
    out = [0.0] * len(sig)
    y = 0.0
    const = not isinstance(cutoff, list)
    for i, x in enumerate(sig):
        fc = cutoff if const else cutoff[i]
        a = 1.0 - math.exp(-2.0 * math.pi * fc / SR)
        y += a * (x - y)
        out[i] = y
    return out


def one_pole_hp(sig: list[float], cutoff: float) -> list[float]:
    """One-pole high-pass = signal minus its low-passed self."""
    lp = one_pole_lp(sig, cutoff)
    return [s - l for s, l in zip(sig, lp)]


def sweep(f0: float, f1: float, count: int) -> list[float]:
    """Linear ramp of `count` values from f0 to f1 (for time-varying cutoff)."""
    if count <= 1:
        return [f1] * count
    return [f0 + (f1 - f0) * (i / (count - 1)) for i in range(count)]


def mul(sig: list[float], env: list[float]) -> list[float]:
    return [s * e for s, e in zip(sig, env)]


def gain(sig: list[float], g: float) -> list[float]:
    return [s * g for s in sig]


def mix(*sigs: list[float]) -> list[float]:
    """Sum signals of possibly-different lengths (zero-padded to the longest)."""
    n = max(len(s) for s in sigs)
    out = [0.0] * n
    for s in sigs:
        for i, v in enumerate(s):
            out[i] += v
    return out


def normalize(sig: list[float], peak: float) -> list[float]:
    m = max((abs(s) for s in sig), default=0.0)
    if m < 1e-9:
        return sig
    g = peak / m
    return [s * g for s in sig]


def write_wav(name: str, sig: list[float]) -> None:
    path = os.path.join(OUT_DIR, name)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SR)
        frames = bytearray()
        for s in sig:
            v = int(round(max(-1.0, min(1.0, s)) * 32767.0))
            frames += struct.pack("<h", v)
        w.writeframes(bytes(frames))
    print(f"  {name:24s} {len(sig)/SR*1000:6.0f} ms  {len(sig)*2:7d} bytes")


# ---------------------------------------------------------------------------
# The cues. Each returns a (pre-normalization) signal; durations/envelopes are
# the spec from docs/plans/COMPLETION_PLAN.md Task 5.3.
# ---------------------------------------------------------------------------


def cue_throw(rng: random.Random) -> list[float]:
    """140 ms whoosh: white noise through a one-pole LP sweeping 6 kHz → 1.2 kHz,
    exp decay tau=45 ms. The fang leaving the hand."""
    dur = 0.140
    nz = one_pole_hp(noise(dur, rng), 200.0)  # trim rumble
    cutoff = sweep(6000.0, 1200.0, n_samples(dur))
    body = one_pole_lp(nz, cutoff)
    sig = mul(body, exp_env(dur, 0.045))
    return attack(sig, 1.5)


def cue_throw_empowered(rng: random.Random) -> list[float]:
    """A perfect-catch throw: the base whoosh plus an 880 Hz sine ping (60 ms,
    -6 dB) so an empowered launch reads brighter and 'charged'."""
    base = cue_throw(rng)
    ping = mul(sine(880.0, 0.060), exp_env(0.060, 0.030))
    ping = gain(attack(ping, 2.0), 10.0 ** (-6.0 / 20.0))
    return mix(base, ping)


def cue_ricochet(rng: random.Random) -> list[float]:
    """70 ms hard click: 2.2 kHz square click + a short noise tail, exp
    tau=18 ms. Bone-on-stone bounce off a wall or pyre."""
    dur = 0.070
    click = mul(square(2200.0, dur), exp_env(dur, 0.018))
    tail = mul(one_pole_hp(noise(dur, rng), 1500.0), exp_env(dur, 0.012))
    sig = mix(gain(click, 0.8), gain(tail, 0.5))
    return attack(sig, 0.3)


def cue_shatter(rng: random.Random) -> list[float]:
    """350 ms collapse: dense noise burst low-passed at 3 kHz over a pitch-drop
    body (180 → 60 Hz sine), exp tau=90 ms. A bone pyre breaking."""
    dur = 0.350
    burst = mul(one_pole_lp(noise(dur, rng), 3000.0), exp_env(dur, 0.090))
    # Pitch-drop body: instantaneous frequency 180 -> 60 Hz, integrated phase.
    body = []
    phase = 0.0
    for i in range(n_samples(dur)):
        t = i / SR
        f = 180.0 + (60.0 - 180.0) * (t / dur)
        phase += 2.0 * math.pi * f / SR
        body.append(math.sin(phase))
    body = mul(body, exp_env(dur, 0.110))
    sig = mix(gain(burst, 0.8), gain(body, 0.7))
    return attack(sig, 0.5)


def cue_catch(rng: random.Random) -> list[float]:
    """90 ms: a reversed-envelope noise swell rising into a 1.4 kHz tick. The
    snap of the fang into the hand."""
    dur = 0.090
    n = n_samples(dur)
    swell_env = [(i / n) ** 2 for i in range(n)]  # reversed env: silent → loud
    swell = mul(one_pole_lp(noise(dur, rng), 2500.0), swell_env)
    tick = mul(sine(1400.0, 0.020), exp_env(0.020, 0.008))
    tick_padded = silence(dur - 0.020) + attack(tick, 0.3)
    return mix(gain(swell, 0.7), gain(tick_padded, 0.8))


def cue_catch_perfect(_rng: random.Random) -> list[float]:
    """250 ms inharmonic bell — sines at 523/682/941 Hz (fixed, non-integer
    ratios for a metallic glint), exp tau=120 ms. The signature reward."""
    dur = 0.250
    partials = [(523.0, 1.0), (682.0, 0.6), (941.0, 0.4)]
    env = exp_env(dur, 0.120)
    bell = silence(dur)
    for f, amp in partials:
        bell = mix(bell, gain(mul(sine(f, dur), env), amp))
    return attack(bell, 2.0)


def cue_kill(rng: random.Random) -> list[float]:
    """400 ms: a 55 Hz sub thump (tau=150 ms) under a wet noise splat band-passed
    400–2500 Hz (tau=80 ms). The one-hit kill — felt in the chest."""
    dur = 0.400
    sub = mul(sine(55.0, dur), exp_env(dur, 0.150))
    raw = noise(dur, rng)
    band = one_pole_lp(one_pole_hp(raw, 400.0), 2500.0)
    splat = mul(band, exp_env(dur, 0.080))
    # A 1-sample-ish transient click for the initial impact crack.
    click = mul(noise(0.004, rng), exp_env(0.004, 0.0015))
    click = click + silence(dur - 0.004)
    sig = mix(gain(sub, 1.0), gain(splat, 0.7), gain(click, 0.6))
    return attack(sig, 0.5)


def cue_countdown_toll(_rng: random.Random) -> list[float]:
    """600 ms low bell: 110 Hz + 220 Hz sines, slow tau=300 ms. Plays on 3/2/1;
    the GO is this same file at playback speed 1.25 (≈ a major-third up)."""
    dur = 0.600
    env = exp_env(dur, 0.300)
    toll = mix(mul(sine(110.0, dur), env), gain(mul(sine(220.0, dur), env), 0.5))
    return attack(toll, 3.0)


def cue_round_over_sting(_rng: random.Random) -> list[float]:
    """800 ms descending minor dyad — two saws gliding 220→165 Hz (a minor third
    apart in motion), low-passed for a mournful, dust-settling fall."""
    dur = 0.800
    n = n_samples(dur)

    def glide_saw(f0: float, f1: float) -> list[float]:
        out = []
        phase = 0.0
        for i in range(n):
            t = i / SR
            f = f0 + (f1 - f0) * (t / dur)
            phase += f / SR
            out.append(2.0 * (phase - math.floor(phase + 0.5)))
        return out

    dyad = mix(glide_saw(220.0, 165.0), gain(glide_saw(277.0, 208.0), 0.7))
    dyad = one_pole_lp(dyad, 1600.0)
    sig = mul(dyad, exp_env(dur, 0.450))
    return attack(sig, 4.0)


def _chime(f_start: float, f_end: float) -> list[float]:
    """120 ms two-tone chime gliding f_start → f_end with a soft bell decay."""
    dur = 0.120
    n = n_samples(dur)
    out = []
    phase = 0.0
    for i in range(n):
        t = i / SR
        f = f_start + (f_end - f_start) * (t / dur)
        phase += 2.0 * math.pi * f / SR
        out.append(math.sin(phase) + 0.4 * math.sin(2.0 * phase))
    sig = mul(out, exp_env(dur, 0.060))
    return attack(sig, 2.0)


def cue_pickup_spawn(_rng: random.Random) -> list[float]:
    """120 ms ascending chime 440 → 660 Hz — a modifier blooms into the arena."""
    return _chime(440.0, 660.0)


def cue_pickup_collect(_rng: random.Random) -> list[float]:
    """120 ms descending chime 660 → 440 Hz — the modifier is taken up."""
    return _chime(660.0, 440.0)


def _supersaw(base: float, dur: float,
              detunes: tuple[float, ...] = (-0.5, -0.25, 0.0, 0.25, 0.5)) -> list[float]:
    """A detuned saw cluster — the 'fat analog' width. Detune offsets are
    multiples of 0.125 Hz so every voice still completes a whole number of
    cycles in an 8 s loop and the seam stays phase-continuous (no click)."""
    g = 1.0 / len(detunes)
    return mix(*[gain(saw(base + d, dur), g) for d in detunes])


def _saturate(sig: list[float], drive: float) -> list[float]:
    """Soft analog saturation (tanh) — fattens and warms, rounds the saw edges
    the way an overdriven analog VCA/filter does."""
    return [math.tanh(s * drive) for s in sig]


def cue_ambient_loop(rng: random.Random) -> list[float]:
    """8 s seamless bed — a FAT, warm, detuned analog synth (Regular-Show
    title-card register), not the old thin sub-drone. A detuned supersaw chord
    (root A1 + fifth E2 + octave A2), tanh-saturated for analog grit and run
    through a warm-but-open filter, over a sine sub for weight, plus a subtle
    raised-cosine air swell. Every oscillator completes whole cycles in 8 s
    (freqs on a 0.125 Hz grid) so the loop point is phase-continuous. -18 dBFS."""
    dur = 8.0
    n = n_samples(dur)
    # Detuned analog chord — power-chord voicing for fatness.
    chord = mix(
        _supersaw(55.0, dur),                  # A1 root
        gain(_supersaw(82.5, dur), 0.7),       # E2 fifth
        gain(_supersaw(110.0, dur), 0.5),      # A2 octave
    )
    chord = one_pole_lp(chord, 1500.0)         # warm, but open (was a muffled 700)
    chord = _saturate(chord, 1.7)              # analog fatten/warm
    chord = one_pole_lp(chord, 2400.0)         # tame the saturation fizz
    sub = gain(sine(27.5, dur), 0.55)          # A0 sub — weight under the chord
    # Subtle airy swell for movement; raised cosine is 0 at both ends (no seam click).
    swell_env = [0.5 - 0.5 * math.cos(2.0 * math.pi * (i / n)) for i in range(n)]
    swell = mul(one_pole_lp(noise(dur, rng), 1100.0), swell_env)
    bed = mix(gain(chord, 0.62), sub, gain(swell, 0.22))
    return bed


# ---------------------------------------------------------------------------
# Driver. Each SFX gets its own rng stream derived from the master seed so the
# files are order-independent (adding a cue doesn't perturb the others' noise).
# ---------------------------------------------------------------------------

CUES = [
    ("throw.wav", cue_throw, SFX_PEAK),
    ("throw_empowered.wav", cue_throw_empowered, SFX_PEAK),
    ("ricochet.wav", cue_ricochet, SFX_PEAK),
    ("shatter.wav", cue_shatter, SFX_PEAK),
    ("catch.wav", cue_catch, SFX_PEAK),
    ("catch_perfect.wav", cue_catch_perfect, SFX_PEAK),
    ("kill.wav", cue_kill, SFX_PEAK),
    ("countdown_toll.wav", cue_countdown_toll, SFX_PEAK),
    ("round_over_sting.wav", cue_round_over_sting, SFX_PEAK),
    ("pickup_spawn.wav", cue_pickup_spawn, SFX_PEAK),
    ("pickup_collect.wav", cue_pickup_collect, SFX_PEAK),
    ("ambient_loop.wav", cue_ambient_loop, AMBIENT_PEAK),
]


def main() -> None:
    os.makedirs(OUT_DIR, exist_ok=True)
    print(f"Generating {len(CUES)} cues → {os.path.relpath(OUT_DIR, ROOT)}/")
    for idx, (name, fn, peak) in enumerate(CUES):
        # Per-cue deterministic stream: master seed mixed with the cue index.
        rng = random.Random(SEED ^ (0x9E3779B9 * (idx + 1) & 0xFFFFFFFF))
        sig = fn(rng)
        sig = normalize(sig, peak)
        write_wav(name, sig)
    print("done.")


if __name__ == "__main__":
    main()
