#!/usr/bin/env python3
"""Generate 2-Top's synthesized audio — the Bone Cathedral, scored in 1984.

Every sound in the game comes out of this file: an 80s analog-synth palette
(fat detuned saws, resonant filter sweeps, PWM pulses, LinnDrum-style gated
drums, dotted-eighth echoes) in the John Carpenter horror-synth lineage —
minor-key ostinatos and dread, not sunset-drive synthwave. Two real music
loops (a moody title theme, a driving match groove), a heartbeat for the
match-point ritual, and twenty-odd event cues, all synthesized from first
principles with the Python standard library only (`wave`, `math`, `struct`,
a seeded `random.Random`). Deterministic: regenerating on any machine yields
byte-identical WAVs, the same discipline the art generator follows. No
samples, no external DSP libraries, no network.

Format: 22050 Hz, mono, 16-bit PCM. One-shot SFX peak at -3 dBFS; the music
loops master at -12 dBFS so they sit under gameplay cues at the default bus
levels (the app squares the 0..1 volume sliders into gain — a perceptual
taper — so the in-file peaks here are chosen against slider^2 x peak).
Music loops are seamless by construction: each track renders with a tail
margin and the tail (echo/reverb/release ring-outs) is FOLDED back onto the
loop's start, so the seam is a real mix, not a splice.

Cue → game event wiring lives in `crates/app/src/audio.rs` (GameAudioPlugin),
fired off the same render-side sim-event edges the effect sprites use. The GO
toll is not a separate file: it is `countdown_toll.wav` at speed 1.25.

Run: `python3 scripts/generate_audio.py`  → writes assets/audio/*.wav
(pure-Python per-sample DSP; a full regeneration takes on the order of a
minute — it is an offline atelier tool, not a build step).
"""

from __future__ import annotations

import math
import os
import random
import struct
import wave

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
OUT_DIR = os.path.join(ROOT, "assets", "audio")

SR = 22050  # sample rate (Hz) — chunky, dry, honest to the pixel pole.

# Seed = ASCII bytes of "2TOP". Fixed seed → reproducible noise beds.
SEED = 0x32544F50

# Target peak levels (linear amplitude from dBFS).
SFX_PEAK = 10.0 ** (-3.0 / 20.0)     # ~0.708 — cues punch
MUSIC_PEAK = 10.0 ** (-12.0 / 20.0)  # ~0.251 — beds sit under the cues


# ---------------------------------------------------------------------------
# DSP toolkit (stdlib only). A "signal" is a list[float] in [-1, 1]-ish;
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
        ph = freq * i / SR
        out.append(2.0 * (ph - math.floor(ph + 0.5)))
    return out


def square(freq: float, seconds: float) -> list[float]:
    return [1.0 if s >= 0.0 else -1.0 for s in sine(freq, seconds)]


def pulse(freq: float, seconds: float, width: float = 0.5,
          pwm_rate: float = 0.0, pwm_depth: float = 0.0) -> list[float]:
    """Pulse oscillator with optional pulse-width modulation — the slow PWM
    shimmer is half of what people hear as 'a Juno'."""
    out = []
    n = n_samples(seconds)
    for i in range(n):
        t = i / SR
        w = width + pwm_depth * math.sin(2.0 * math.pi * pwm_rate * t)
        ph = (freq * t) % 1.0
        out.append(1.0 if ph < w else -1.0)
    return out


def glide(shape: str, f0: float, f1: float, seconds: float) -> list[float]:
    """Phase-integrated frequency glide (portamento) for 'saw'/'sine'/'square'."""
    n = n_samples(seconds)
    out = []
    phase = 0.0
    for i in range(n):
        f = f0 + (f1 - f0) * (i / max(1, n - 1))
        phase += f / SR
        if shape == "saw":
            out.append(2.0 * (phase - math.floor(phase + 0.5)))
        elif shape == "square":
            out.append(1.0 if (phase % 1.0) < 0.5 else -1.0)
        else:
            out.append(math.sin(2.0 * math.pi * phase))
    return out


def noise(seconds: float, rng: random.Random) -> list[float]:
    return [rng.uniform(-1.0, 1.0) for _ in range(n_samples(seconds))]


def exp_env(seconds: float, tau: float) -> list[float]:
    return [math.exp(-(i / SR) / tau) for i in range(n_samples(seconds))]


def adsr(total_s: float, a: float, d: float, s_level: float, r: float) -> list[float]:
    """Classic ADSR over `total_s`: linear attack, exponential-ish decay to the
    sustain level, hold, then a release ramp filling the final `r` seconds."""
    n = n_samples(total_s)
    na, nd, nr = n_samples(a), n_samples(d), n_samples(r)
    out = [0.0] * n
    for i in range(n):
        if i < na:
            v = i / max(1, na)
        elif i < na + nd:
            k = (i - na) / max(1, nd)
            v = 1.0 + (s_level - 1.0) * (1.0 - (1.0 - k) ** 2)
        elif i < n - nr:
            v = s_level
        else:
            k = (i - (n - nr)) / max(1, nr)
            v = s_level * (1.0 - k) ** 2
        out[i] = v
    return out


def attack(sig: list[float], ms: float) -> list[float]:
    """Linear fade-in over the first `ms` to kill the onset click."""
    a = max(1, n_samples(ms / 1000.0))
    out = list(sig)
    for i in range(min(a, len(out))):
        out[i] *= i / a
    return out


def one_pole_lp(sig: list[float], cutoff) -> list[float]:
    """One-pole low-pass; `cutoff` is a constant Hz or per-sample list."""
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
    lp = one_pole_lp(sig, cutoff)
    return [s - l for s, l in zip(sig, lp)]


def svf_lp(sig: list[float], cutoff, q: float) -> list[float]:
    """Resonant 2-pole low-pass (topology-preserving state-variable filter,
    Simper's ZDF form — stable to Nyquist). THE analog voice: at q >~ 2 the
    sweep squelches; at q >~ 6 a short impulse rings it like a tuned drum.
    `cutoff` is a constant Hz or a per-sample list for sweeps."""
    out = [0.0] * len(sig)
    ic1 = ic2 = 0.0
    k = 1.0 / max(0.1, q)
    const = not isinstance(cutoff, list)
    g = math.tan(math.pi * min(9500.0, max(10.0, cutoff)) / SR) if const else 0.0
    for i, x in enumerate(sig):
        if not const:
            g = math.tan(math.pi * min(9500.0, max(10.0, cutoff[i])) / SR)
        a1 = 1.0 / (1.0 + g * (g + k))
        a2 = g * a1
        a3 = g * a2
        v3 = x - ic2
        v1 = a1 * ic1 + a2 * v3
        v2 = ic2 + a2 * ic1 + a3 * v3
        ic1 = 2.0 * v1 - ic1
        ic2 = 2.0 * v2 - ic2
        out[i] = v2
    return out


def sweep(f0: float, f1: float, count: int) -> list[float]:
    if count <= 1:
        return [f1] * count
    return [f0 + (f1 - f0) * (i / (count - 1)) for i in range(count)]


def exp_sweep(f0: float, f1: float, count: int) -> list[float]:
    """Exponential cutoff ramp — filter sweeps read as motion in log-Hz."""
    if count <= 1:
        return [f1] * count
    r = f1 / f0
    return [f0 * (r ** (i / (count - 1))) for i in range(count)]


def mul(sig: list[float], env: list[float]) -> list[float]:
    return [s * e for s, e in zip(sig, env)]


def gain(sig: list[float], g: float) -> list[float]:
    return [s * g for s in sig]


def mix(*sigs: list[float]) -> list[float]:
    n = max(len(s) for s in sigs)
    out = [0.0] * n
    for s in sigs:
        for i, v in enumerate(s):
            out[i] += v
    return out


def _saturate(sig: list[float], drive: float) -> list[float]:
    """Soft analog saturation (tanh) — fattens and warms like a pushed VCA."""
    return [math.tanh(s * drive) for s in sig]


def echo(sig: list[float], time_s: float, fb: float, wet: float,
         tail_s: float = 0.0) -> list[float]:
    """Feedback delay — the dotted-eighth echo is the Carpenter signature.
    Output extends `tail_s` past the input so repeats can ring out."""
    d = max(1, n_samples(time_s))
    n_out = len(sig) + n_samples(tail_s)
    e = [0.0] * n_out
    for i in range(n_out):
        dry_tap = sig[i - d] if d <= i and (i - d) < len(sig) else 0.0
        fb_tap = e[i - d] * fb if i >= d else 0.0
        e[i] = dry_tap + fb_tap
    out = [0.0] * n_out
    for i in range(n_out):
        out[i] = (sig[i] if i < len(sig) else 0.0) + wet * e[i]
    return out


def gated_verb(sig: list[float], rng: random.Random, room_s: float = 0.10,
               gate_s: float = 0.13, taps: int = 14, damp: float = 3200.0,
               level: float = 0.7) -> list[float]:
    """Gated reverb — the 80s drum sound: a burst of dense early reflections
    chopped dead by a gate before it can bloom into a tail."""
    tap_list = [(rng.uniform(0.003, room_s), rng.uniform(0.35, 1.0)) for _ in range(taps)]
    n_wet = len(sig) + n_samples(room_s)
    wet = [0.0] * n_wet
    for t, a in tap_list:
        off = n_samples(t)
        amp = a * math.exp(-t / (room_s * 0.6))
        for i, v in enumerate(sig):
            j = i + off
            if j < n_wet:
                wet[j] += v * amp / taps * 4.0
    wet = one_pole_lp(wet, damp)
    n_gate = n_samples(gate_s)
    n_close = n_samples(0.004)
    for i in range(len(wet)):
        if i >= n_gate + n_close:
            wet[i] = 0.0
        elif i >= n_gate:
            k = (i - n_gate) / n_close
            wet[i] *= 0.5 + 0.5 * math.cos(math.pi * k)
    return mix(sig, gain(wet, level))


def chorus(sig: list[float], rate: float = 0.7, depth_ms: float = 5.0,
           wet: float = 0.4) -> list[float]:
    """Modulated short delay — analog ensemble thickness (mono, still works)."""
    base = n_samples(0.012)
    depth = n_samples(depth_ms / 1000.0)
    out = list(sig)
    for i in range(len(sig)):
        m = base + depth * (0.5 + 0.5 * math.sin(2.0 * math.pi * rate * i / SR))
        j = i - m
        j0 = int(math.floor(j))
        frac = j - j0
        if j0 >= 0 and j0 + 1 < len(sig):
            out[i] += wet * (sig[j0] * (1.0 - frac) + sig[j0 + 1] * frac)
    return out


def normalize(sig: list[float], peak: float) -> list[float]:
    m = max((abs(s) for s in sig), default=0.0)
    if m < 1e-9:
        return sig
    return [s * (peak / m) for s in sig]


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
    rms = math.sqrt(sum(s * s for s in sig) / max(1, len(sig)))
    print(f"  {name:24s} {len(sig)/SR*1000:7.0f} ms  {len(sig)*2:8d} B  rms {20*math.log10(max(rms,1e-9)):6.1f} dB")


# ---------------------------------------------------------------------------
# Musical helpers: note names, fat oscillators, sequenced voices.
# ---------------------------------------------------------------------------

_NOTE_IDX = {"C": 0, "C#": 1, "D": 2, "D#": 3, "E": 4, "F": 5,
             "F#": 6, "G": 7, "G#": 8, "A": 9, "A#": 10, "B": 11}


def hz(note: str) -> float:
    """'A2' → 110.0. A4 = 440."""
    name, octave = note[:-1], int(note[-1])
    semis = (_NOTE_IDX[name] - 9) + (octave - 4) * 12
    return 440.0 * (2.0 ** (semis / 12.0))


def fat_saw(freq: float, seconds: float,
            cents: tuple[float, ...] = (-9.0, -4.0, 0.0, 4.0, 9.0)) -> list[float]:
    """Detuned saw stack (cents-based) — the fat analog unison."""
    g = 1.0 / len(cents)
    return mix(*[gain(saw(freq * (2.0 ** (c / 1200.0)), seconds), g) for c in cents])


def place(dst: list[float], at_s: float, src: list[float], level: float = 1.0) -> None:
    """Add `src` into `dst` starting at `at_s`; clipped to dst's length (the
    music timelines carry a tail margin that fold_loop wraps around)."""
    start = int(round(at_s * SR))
    n = len(dst)
    for i, v in enumerate(src):
        j = start + i
        if 0 <= j < n:
            dst[j] += v * level


def fold_loop(sig: list[float], loop_n: int) -> list[float]:
    """Seamless loop: everything past `loop_n` (echo/release tails) is folded
    back onto the start, so the seam is a mix, not a splice."""
    out = sig[:loop_n]
    for i, v in enumerate(sig[loop_n:]):
        out[i % loop_n] += v
    return out


def pump_env(n_total: int, beat_times: list[float], depth: float = 0.4,
             recover_s: float = 0.35) -> list[float]:
    """Baked sidechain pump: dip to (1-depth) at each beat, exponential-feel
    recovery — the bass 'breathes' against the kick, very 80s."""
    env = [1.0] * n_total
    n_rec = n_samples(recover_s)
    for t in beat_times:
        start = int(round(t * SR))
        for i in range(n_rec):
            j = start + i
            if 0 <= j < n_total:
                k = (i / n_rec) ** 1.6
                v = 1.0 - depth + depth * k
                if v < env[j]:
                    env[j] = v
    return env


def bass_pluck(freq: float, gate_s: float) -> list[float]:
    """Analog bass pluck: saw + square sub an octave down, resonant cutoff
    envelope, saturated. Tail = gate + a short release."""
    dur = gate_s + 0.05
    body = mix(gain(saw(freq, dur), 0.8), gain(square(freq * 0.5, dur), 0.5))
    cut = exp_sweep(1400.0, 160.0, n_samples(dur))
    body = svf_lp(body, cut, 1.6)
    env = adsr(dur, 0.003, gate_s * 0.7, 0.35, 0.045)
    return _saturate(mul(body, env), 1.5)


def arp_note(freq: float, gate_s: float, cutoff: float, q: float = 2.0) -> list[float]:
    """PWM pulse pluck for arpeggios; cutoff supplied by the caller's LFO."""
    dur = gate_s + 0.04
    body = pulse(freq, dur, width=0.42, pwm_rate=5.0, pwm_depth=0.08)
    body = svf_lp(body, min(9000.0, cutoff), q)
    env = adsr(dur, 0.002, gate_s * 0.6, 0.25, 0.035)
    return mul(body, env)


def pad_chord(freqs: list[float], dur_s: float, lp_base: float, lp_lfo: float,
              lfo_rate: float, level_per_note: float = 0.5) -> list[float]:
    """Detuned supersaw pad with a slow filter wobble and chorus."""
    stack = mix(*[gain(fat_saw(f, dur_s), level_per_note) for f in freqs])
    n = n_samples(dur_s)
    cut = [lp_base + lp_lfo * math.sin(2.0 * math.pi * lfo_rate * i / SR) for i in range(n)]
    stack = svf_lp(stack, cut, 0.9)
    env = adsr(dur_s, 0.9, 0.5, 0.85, 1.2)
    return chorus(mul(stack, env), rate=0.5, depth_ms=6.0, wet=0.45)


def stab(freqs: list[float], gate_s: float) -> list[float]:
    """Brass-adjacent saw stab: fast attack, resonant closing filter."""
    dur = gate_s + 0.12
    stack = mix(*[gain(fat_saw(f, dur, cents=(-6.0, 0.0, 6.0)), 0.6) for f in freqs])
    cut = exp_sweep(3600.0, 700.0, n_samples(dur))
    stack = svf_lp(stack, cut, 2.2)
    env = adsr(dur, 0.004, gate_s * 0.8, 0.4, 0.1)
    return _saturate(mul(stack, env), 1.3)


# ---- LinnDrum-ish kit -------------------------------------------------------


def drum_kick() -> list[float]:
    dur = 0.16
    n = n_samples(dur)
    freqs = exp_sweep(150.0, 44.0, n)
    phase = 0.0
    body = []
    for i in range(n):
        phase += 2.0 * math.pi * freqs[i] / SR
        body.append(math.sin(phase) * math.exp(-i / (SR * 0.055)))
    click = mul(square(3200.0, 0.003), exp_env(0.003, 0.001))
    return _saturate(mix(body, gain(click, 0.25)), 1.6)


def drum_snare(rng: random.Random) -> list[float]:
    body = mul(glide("sine", 196.0, 160.0, 0.07), exp_env(0.07, 0.03))
    snap = one_pole_hp(one_pole_lp(noise(0.19, rng), 6200.0), 900.0)
    snap = mul(snap, exp_env(0.19, 0.05))
    dry = mix(gain(body, 0.8), gain(snap, 0.9))
    return _saturate(gated_verb(dry, rng, room_s=0.11, gate_s=0.12, level=0.85), 1.3)


def drum_clap(rng: random.Random) -> list[float]:
    """LinnDrum clap: three tight pre-bursts then the main body."""
    out = silence(0.24)
    for at, amp, tau in ((0.0, 0.55, 0.006), (0.011, 0.6, 0.006),
                         (0.023, 0.65, 0.007), (0.034, 1.0, 0.045)):
        b = one_pole_hp(one_pole_lp(noise(0.16, rng), 4200.0), 1000.0)
        place(out, at, mul(b, exp_env(0.16, tau)), amp)
    return out


def drum_hat(rng: random.Random, open_hat: bool = False) -> list[float]:
    dur = 0.13 if open_hat else 0.03
    tau = 0.045 if open_hat else 0.008
    return mul(one_pole_hp(noise(dur, rng), 6500.0), exp_env(dur, tau))


# ---------------------------------------------------------------------------
# One-shot cues (all -3 dBFS at write time).
# ---------------------------------------------------------------------------


def cue_throw(rng: random.Random) -> list[float]:
    """The fang leaves the hand: a saw zap gliding down an octave through a
    closing resonant sweep, with a noise whoosh riding on top."""
    dur = 0.15
    zap = glide("saw", 220.0, 105.0, dur)
    zap = svf_lp(zap, exp_sweep(3800.0, 420.0, n_samples(dur)), 2.4)
    zap = mul(zap, exp_env(dur, 0.05))
    whoosh = one_pole_hp(noise(dur, rng), 900.0)
    whoosh = mul(one_pole_lp(whoosh, exp_sweep(6000.0, 1200.0, n_samples(dur))),
                 exp_env(dur, 0.04))
    return attack(mix(gain(zap, 0.9), gain(whoosh, 0.5)), 1.0)


def cue_throw_empowered(rng: random.Random) -> list[float]:
    """Empowered launch: the base zap plus a detuned fifth stab and one
    slapback echo — brighter, charged, a little smug."""
    base = cue_throw(rng)
    chord = mul(mix(fat_saw(hz("A4"), 0.1, cents=(-7.0, 0.0, 7.0)),
                    gain(fat_saw(hz("E5"), 0.1, cents=(-7.0, 0.0, 7.0)), 0.6)),
                exp_env(0.1, 0.045))
    chord = svf_lp(chord, 4200.0, 1.5)
    return mix(base, echo(gain(attack(chord, 1.5), 0.5), 0.09, 0.35, 0.5, tail_s=0.2))


def cue_ricochet(rng: random.Random) -> list[float]:
    """Wall bounce: a 4 ms noise impulse rings a high-Q filter like a tuned
    bone — 'tonk' — plus a bright transient click."""
    imp = mul(noise(0.004, rng), exp_env(0.004, 0.002)) + silence(0.076)
    ring = svf_lp(imp, 1900.0, 8.0)
    click = mul(one_pole_hp(noise(0.012, rng), 3000.0), exp_env(0.012, 0.004))
    return attack(mix(gain(ring, 3.0), gain(click, 0.5)), 0.2)


def cue_shatter(rng: random.Random) -> list[float]:
    """A pyre collapsing: noise crash through a falling resonant sweep over a
    sub drop, gated so the debris cuts off 80s-style."""
    dur = 0.36
    crash = svf_lp(noise(dur, rng), exp_sweep(5200.0, 300.0, n_samples(dur)), 1.3)
    crash = mul(crash, exp_env(dur, 0.09))
    drop = mul(glide("sine", 130.0, 46.0, dur), exp_env(dur, 0.12))
    sig = gated_verb(mix(gain(crash, 0.85), gain(drop, 0.8)), rng,
                     room_s=0.12, gate_s=0.16, level=0.6)
    return attack(sig, 0.5)


def cue_catch(rng: random.Random) -> list[float]:
    """The snap into the hand: a reverse swell rising into a filter-pinged
    pluck at E5."""
    swell_n = n_samples(0.07)
    swell = mul(one_pole_lp(noise(0.07, rng), 2600.0),
                [(i / swell_n) ** 2 for i in range(swell_n)])
    imp = mul(noise(0.003, rng), exp_env(0.003, 0.0015)) + silence(0.06)
    ping = gain(svf_lp(imp, hz("E5"), 7.0), 2.6)
    return mix(gain(swell, 0.6), gain(place_after(swell, ping), 0.9))


def place_after(before: list[float], sig: list[float]) -> list[float]:
    """Pad `sig` to start where `before` ends (tiny sequencing helper)."""
    return [0.0] * len(before) + sig


def cue_catch_perfect(_rng: random.Random) -> list[float]:
    """The signature reward: a fast three-note PWM arp up the octave
    (A5-E6-A6) with a feedback echo shimmering off it."""
    seq = silence(0.5)
    for i, note in enumerate(("A5", "E6", "A6")):
        n = mul(pulse(hz(note), 0.07, width=0.4, pwm_rate=6.0, pwm_depth=0.06),
                exp_env(0.07, 0.03))
        place(seq, i * 0.045, svf_lp(attack(n, 1.0), 6800.0, 1.4), 1.0 - 0.15 * i)
    return echo(seq, 0.13, 0.4, 0.4, tail_s=0.35)


def cue_kill(rng: random.Random) -> list[float]:
    """The one-hit kill — the 80s action hit: sub drop, noise crack, a gated
    snare-burst body, and a downward resonant zap. Felt in the chest."""
    dur = 0.45
    sub = _saturate(mul(glide("sine", 110.0, 38.0, dur), exp_env(dur, 0.16)), 1.6)
    crack = mul(noise(0.006, rng), exp_env(0.006, 0.002))
    burst = one_pole_hp(one_pole_lp(noise(0.2, rng), 5200.0), 500.0)
    burst = gated_verb(mul(burst, exp_env(0.2, 0.06)), rng,
                       room_s=0.12, gate_s=0.14, level=0.9)
    zap = glide("saw", 220.0, 55.0, 0.22)
    zap = mul(svf_lp(zap, exp_sweep(2000.0, 180.0, n_samples(0.22)), 3.0),
              exp_env(0.22, 0.07))
    return attack(mix(sub, gain(crack, 0.7), gain(burst, 0.6), gain(zap, 0.7)), 0.4)


def cue_countdown_toll(_rng: random.Random) -> list[float]:
    """3/2/1 toll: a deep detuned-square knell through a resonant band, with a
    sub thump under it. GO is this file played at 1.25x speed."""
    dur = 0.6
    knell = mix(square(hz("A2"), dur), gain(square(hz("E3") * 1.003, dur), 0.5))
    knell = svf_lp(knell, 900.0, 2.0)
    knell = mul(knell, exp_env(dur, 0.28))
    thump = mul(glide("sine", 90.0, 52.0, 0.18), exp_env(0.18, 0.06))
    return attack(mix(gain(knell, 0.8), gain(thump, 0.7)), 3.0)


def cue_round_over_sting(_rng: random.Random) -> list[float]:
    """Round over: a mournful supersaw dyad gliding down a fourth through a
    closing sweep, dotted-eighth echoes trailing the dust."""
    dur = 0.9
    n = n_samples(dur)

    def fat_glide(f0: float, f1: float) -> list[float]:
        voices = []
        for c in (-8.0, 0.0, 8.0):
            k = 2.0 ** (c / 1200.0)
            voices.append(gain(glide("saw", f0 * k, f1 * k, dur), 0.33))
        return mix(*voices)

    dyad = mix(fat_glide(hz("A3"), hz("E3")), gain(fat_glide(hz("C4"), hz("G3")), 0.7))
    dyad = svf_lp(dyad, exp_sweep(2400.0, 380.0, n), 1.6)
    sig = mul(dyad, exp_env(dur, 0.4))
    return echo(attack(sig, 4.0), 0.28, 0.35, 0.3, tail_s=0.6)


def cue_match_win_sting(rng: random.Random) -> list[float]:
    """Match won: two big minor stabs an octave apart, gated reverb on the
    hits, a sub drop for weight, echoes carrying the hall. Dark triumph."""
    seq = silence(1.5)
    hit1 = stab([hz("A3"), hz("C4"), hz("E4")], 0.22)
    hit2 = stab([hz("A4"), hz("C5"), hz("E5")], 0.3)
    place(seq, 0.0, gated_verb(hit1, rng, level=0.7), 0.95)
    place(seq, 0.30, gated_verb(hit2, rng, level=0.7), 1.0)
    place(seq, 0.30, mul(glide("sine", 100.0, 41.0, 0.4), exp_env(0.4, 0.14)), 0.8)
    return echo(seq, 0.28, 0.4, 0.35, tail_s=0.7)


def _chirp(f0: float, f1: float) -> list[float]:
    """110 ms PWM chirp for the pickup pair."""
    dur = 0.11
    n = n_samples(dur)
    out = []
    phase = 0.0
    for i in range(n):
        f = f0 + (f1 - f0) * (i / max(1, n - 1))
        phase += f / SR
        w = 0.45 + 0.1 * math.sin(2.0 * math.pi * 7.0 * i / SR)
        out.append(1.0 if (phase % 1.0) < w else -1.0)
    out = svf_lp(out, 5200.0, 1.8)
    sig = mul(out, adsr(dur, 0.003, 0.05, 0.4, 0.03))
    return echo(attack(sig, 1.0), 0.08, 0.3, 0.3, tail_s=0.15)


def cue_pickup_spawn(_rng: random.Random) -> list[float]:
    return _chirp(hz("E5"), hz("A5"))


def cue_pickup_collect(_rng: random.Random) -> list[float]:
    return _chirp(hz("A5"), hz("E5"))


def cue_dash(rng: random.Random) -> list[float]:
    """The dash: a rising zip — saw gliding up an octave through an OPENING
    resonant sweep with an airy noise layer. Fast, gone, weightless."""
    dur = 0.09
    zip_ = glide("saw", hz("A4"), hz("A5"), dur)
    zip_ = svf_lp(zip_, exp_sweep(1200.0, 5200.0, n_samples(dur)), 2.0)
    air = one_pole_hp(noise(dur, rng), 1800.0)
    sig = mul(mix(gain(zip_, 0.85), gain(air, 0.45)), exp_env(dur, 0.035))
    return attack(sig, 0.6)


def cue_dash_ready(rng: random.Random) -> list[float]:
    """Cooldown refilled: one soft high filter-ping. UI-quiet by design —
    the app also trims it down on playback."""
    imp = mul(noise(0.003, rng), exp_env(0.003, 0.0015)) + silence(0.08)
    return attack(gain(svf_lp(imp, 1320.0, 7.0), 2.4), 0.3)


# Charge riser duration — matches sim::CHARGE_MAX_FRAMES (34) at 60 Hz.
CHARGE_RISER_S = 34.0 / 60.0


def cue_charge_riser(_rng: random.Random) -> list[float]:
    """The wind-up: a low saw drone climbing through an opening resonant
    sweep with an accelerating tremolo — tension that reads across the room.
    Cut off by the app the instant the throw releases."""
    dur = CHARGE_RISER_S
    n = n_samples(dur)
    body = mix(fat_saw(hz("A2"), dur, cents=(-6.0, 0.0, 6.0)),
               gain(sine(hz("A1"), dur), 0.5))
    body = svf_lp(body, exp_sweep(280.0, 3400.0, n), 2.6)
    trem = [1.0 - 0.35 * (0.5 + 0.5 * math.sin(2.0 * math.pi * (4.0 + 10.0 * (i / n) ** 2) * i / SR))
            for i in range(n)]
    env = [0.55 + 0.45 * (i / n) for i in range(n)]
    return attack(mul(mul(body, trem), env), 8.0)


def cue_respawn(_rng: random.Random) -> list[float]:
    """Back from the dead: a PWM fifth swelling up through an opening filter —
    airy, brief, a body re-knitting."""
    dur = 0.32
    n = n_samples(dur)
    chord = mix(pulse(hz("A3"), dur, 0.5, 0.8, 0.12),
                gain(pulse(hz("E4"), dur, 0.5, 1.1, 0.12), 0.7))
    chord = svf_lp(chord, exp_sweep(500.0, 4200.0, n), 1.4)
    env = [(i / n) ** 1.6 * math.exp(-max(0, i - n * 0.8) / (SR * 0.03)) for i in range(n)]
    return attack(mul(chord, env), 2.0)


def cue_sudden_death(rng: random.Random) -> list[float]:
    """The floor starts to fall: a detuned drone sagging down an octave over
    a low rumble and slow gated pulses. Dread, not spectacle."""
    dur = 1.2
    n = n_samples(dur)
    drone = mix(glide("saw", hz("A2"), hz("A1"), dur),
                gain(glide("saw", hz("A2") * 1.01, hz("A1") * 1.01, dur), 0.8))
    drone = svf_lp(drone, exp_sweep(1200.0, 260.0, n), 1.8)
    rumble = mul(one_pole_lp(noise(dur, rng), 160.0), exp_env(dur, 0.5))
    pulse_env = [0.6 + 0.4 * math.cos(2.0 * math.pi * 3.0 * i / SR) for i in range(n)]
    sig = mix(gain(mul(drone, pulse_env), 0.8), gain(rumble, 0.9))
    return attack(mul(sig, exp_env(dur, 0.55)), 6.0)


def cue_menu_tap(_rng: random.Random) -> list[float]:
    """Title menu tap: one quiet square blip through a mild filter."""
    dur = 0.05
    blip = svf_lp(square(hz("A5"), dur), 2600.0, 1.5)
    return attack(mul(blip, exp_env(dur, 0.018)), 0.8)


def cue_taunt(rng: random.Random) -> list[float]:
    """The flex: a cocky two-stab horn figure — short jab, then up a fourth
    with the chest out — slapped with gated verb so the disrespect carries
    across the table. Fires at taunt START (the payout rides
    catch_perfect's arp when the flex completes)."""
    jab = stab([hz("A3"), hz("E4")], 0.10)
    swell = gain(stab([hz("D4"), hz("A4"), hz("D5")], 0.20), 1.15)
    fig = mix(jab + silence(0.36), place_after(jab, silence(0.05) + swell))
    fig = gated_verb(fig, rng, room_s=0.09, gate_s=0.11, level=0.6)
    return attack(fig, 1.5)


def cue_heartbeat_loop(_rng: random.Random) -> list[float]:
    """Match-point ritual bed: a slow human lub-dub, loopable at 1.5 s
    (40 BPM — dread, not exertion). Each thump is a pitch-dropping sine
    knock low-passed to a chest-feel; the 'dub' is softer and closer."""
    total = int(SR * 1.5)
    sig = [0.0] * total

    def thump(at_s: float, f0: float, f1: float, dur_s: float, amp: float) -> None:
        n = int(SR * dur_s)
        start = int(SR * at_s)
        freqs = sweep(f0, f1, n)
        phase = 0.0
        for i in range(n):
            phase += 2.0 * math.pi * freqs[i] / SR
            env = math.exp(-i / (SR * dur_s * 0.28))
            v = math.sin(phase) * env * amp
            j = start + i
            if j < total:
                sig[j] += v

    thump(0.00, 68.0, 46.0, 0.22, 1.0)   # lub
    thump(0.26, 60.0, 42.0, 0.18, 0.72)  # dub
    return one_pole_lp(sig, 140.0)


# ---------------------------------------------------------------------------
# The music. Two loops, one key (A minor), two moods:
#   title_loop — 100 BPM, the cathedral at rest: pads, a patient bass pulse,
#                a plucked echo arp, drums barely breathing.
#   match_loop — 112 BPM, the duel: four-on-floor, gated snare+clap, pumping
#                16th bass, an arpeggio whose filter opens and closes over
#                the whole loop, and a turnaround lick every fourth bar.
# Both render with a tail margin and fold it back onto bar 1 — seamless.
# ---------------------------------------------------------------------------


def music_title(rng: random.Random) -> list[float]:
    bpm = 100.0
    spb = 60.0 / bpm            # seconds per beat
    bar = 4.0 * spb
    bars = 8
    loop_s = bars * bar         # 19.2 s
    loop_n = n_samples(loop_s)
    tl = [0.0] * (loop_n + n_samples(3.0))  # +3 s tail margin, folded later

    # i–VI–III–v, two bars each: the dark-retro workhorse.
    chords = [
        ("A1", ["A2", "C3", "E3"], ["A3", "C4", "E4"]),
        ("F1", ["F2", "A2", "C3"], ["F3", "A3", "C4"]),
        ("C2", ["C3", "E3", "G3"], ["C4", "E4", "G4"]),
        ("E1", ["E2", "G2", "B2"], ["E3", "G3", "B3"]),
    ]

    kick = drum_kick()
    snare = drum_snare(rng)
    hat = drum_hat(rng)

    kick_times: list[float] = []
    for b in range(bars):
        t0 = b * bar
        chord_root, low_triad, high_triad = chords[(b // 2) % 4]

        # Pad: one sustained chord per 2-bar block (placed on even bars).
        if b % 2 == 0:
            pad = pad_chord([hz(x) for x in low_triad] + [hz(high_triad[0])],
                            2.0 * bar, lp_base=950.0, lp_lfo=350.0, lfo_rate=0.18,
                            level_per_note=0.4)
            place(tl, t0, pad, 0.24)

        # Bass: a patient syncopated pulse on the root (8th grid 0,3,4,7).
        for step, lvl in ((0, 1.0), (3, 0.7), (4, 0.85), (7, 0.6)):
            place(tl, t0 + step * spb * 0.5, bass_pluck(hz(chord_root), 0.24), 0.8 * lvl)

        # Arp: plucked 8ths over the chord, dotted-eighth echo applied at
        # the stem level below.
        tones = [hz(low_triad[0]) * 2.0, hz(low_triad[1]) * 2.0,
                 hz(low_triad[2]) * 2.0, hz(high_triad[0]) * 2.0]
        pattern = (0, 2, 1, 3, 1, 2, 0, 2)
        for step in range(8):
            f = tones[pattern[step]]
            place(tl, t0 + step * spb * 0.5, arp_note(f, 0.16, 2100.0), 0.16)

        # Drums: heartbeat-sparse. Kick on 1 and 3; a soft gated snare on
        # beat 3 of every other bar; hats on the offbeats, barely there.
        for beat, lvl in ((0, 0.8), (2, 0.6)):
            place(tl, t0 + beat * spb, kick, lvl * 0.7)
            kick_times.append(t0 + beat * spb)
        if b % 2 == 1:
            place(tl, t0 + 2 * spb, snare, 0.3)
        for eighth in range(8):
            if eighth % 2 == 1:
                place(tl, t0 + eighth * spb * 0.5, hat, 0.1)

    # Dotted-eighth echo over the whole bed (the arp rides it hardest).
    tl = echo(tl, spb * 0.75, 0.35, 0.22, tail_s=0.0)
    # Gentle pump against the kick, then analog glue.
    tl = mul(tl, pump_env(len(tl), kick_times, depth=0.22, recover_s=0.4))
    return fold_loop(_saturate(tl, 1.15), loop_n)


def music_match(rng: random.Random) -> list[float]:
    bpm = 112.0
    spb = 60.0 / bpm
    bar = 4.0 * spb
    bars = 8
    loop_n = n_samples(bars * bar)  # 17.142857 s → exactly 378000 samples
    tl = [0.0] * (loop_n + n_samples(3.0))

    # Four-bar cycle, twice: Am / Am / F / G — static menace, then a lift.
    cycle = [
        ("A1", ["A3", "C4", "E4"]),
        ("A1", ["A3", "C4", "E4"]),
        ("F1", ["F3", "A3", "C4"]),
        ("G1", ["G3", "B3", "D4"]),
    ]

    kick = drum_kick()
    snare = drum_snare(rng)
    clap = drum_clap(rng)
    hat = drum_hat(rng)
    ohat = drum_hat(rng, open_hat=True)

    sixteenth = spb * 0.25
    kick_times: list[float] = []
    n_total = len(tl)

    for b in range(bars):
        t0 = b * bar
        root, triad = cycle[b % 4]

        # Pumping 16th bass: root with octave pops on every fourth step.
        for step in range(16):
            f = hz(root) * (2.0 if step % 4 == 3 else 1.0)
            lvl = (1.0, 0.55, 0.7, 0.85)[step % 4]
            place(tl, t0 + step * sixteenth, bass_pluck(f, 0.1), 0.72 * lvl)

        # Drums: four-on-floor kick; snare+clap on 2 and 4 (gated); closed
        # hats on the offbeat 8ths; a 16th hat run + open hat turning bars
        # 4 and 8 around into the next cycle.
        for beat in range(4):
            place(tl, t0 + beat * spb, kick, 0.95)
            kick_times.append(t0 + beat * spb)
        for beat in (1, 3):
            place(tl, t0 + beat * spb, snare, 0.6)
            place(tl, t0 + beat * spb, clap, 0.45)
        for step in (2, 6, 10, 14):
            place(tl, t0 + step * sixteenth, hat, 0.28)
        if b % 4 == 3:
            for step in (12, 13, 14, 15):
                place(tl, t0 + step * sixteenth, hat, 0.2)
            place(tl, t0 + 14 * sixteenth, ohat, 0.3)

        # Arp: 16ths over the triad + octave, the filter LFO handled below
        # via a per-note cutoff following a full-loop triangle.
        tones = [hz(triad[0]), hz(triad[1]), hz(triad[2]), hz(triad[0]) * 2.0]
        pattern = (0, 1, 2, 3, 2, 3, 1, 2, 0, 2, 1, 3, 2, 1, 3, 2)
        for step in range(16):
            frac_of_loop = (t0 + step * sixteenth) / (bars * bar)
            tri = 1.0 - abs(2.0 * frac_of_loop - 1.0)  # up 4 bars, down 4 — loops
            cutoff = 500.0 + 2300.0 * tri
            lvl = 0.2 if step % 4 == 0 else 0.13
            place(tl, t0 + step * sixteenth,
                  arp_note(tones[pattern[step]], 0.09, cutoff, q=2.2), lvl)

        # Dark pad, low in the mix — space stays open for gameplay cues.
        pad = pad_chord([hz(x) * 0.5 for x in triad], bar,
                        lp_base=750.0, lp_lfo=250.0, lfo_rate=0.25,
                        level_per_note=0.4)
        place(tl, t0, pad, 0.13)

        # Turnaround lick into each cycle restart (bars 4 and 8).
        if b % 4 == 3:
            for i, note in enumerate(("A4", "C5", "B4", "G4")):
                place(tl, t0 + (12 + i) * sixteenth,
                      stab([hz(note)], 0.1), 0.4)

    tl = echo(tl, spb * 0.75, 0.3, 0.16, tail_s=0.0)
    tl = mul(tl, pump_env(n_total, kick_times, depth=0.4, recover_s=0.3))
    return fold_loop(_saturate(tl, 1.25), loop_n)


# ---------------------------------------------------------------------------
# Driver. Each cue gets its own rng stream derived from the master seed AND
# THE CUE NAME (not list position), so adding or reordering cues never
# perturbs the others' noise.
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
    ("match_win_sting.wav", cue_match_win_sting, SFX_PEAK),
    ("pickup_spawn.wav", cue_pickup_spawn, SFX_PEAK),
    ("pickup_collect.wav", cue_pickup_collect, SFX_PEAK),
    ("dash.wav", cue_dash, SFX_PEAK),
    ("dash_ready.wav", cue_dash_ready, SFX_PEAK),
    ("charge_riser.wav", cue_charge_riser, SFX_PEAK),
    ("respawn.wav", cue_respawn, SFX_PEAK),
    ("sudden_death.wav", cue_sudden_death, SFX_PEAK),
    ("menu_tap.wav", cue_menu_tap, SFX_PEAK),
    ("taunt.wav", cue_taunt, SFX_PEAK),
    ("title_loop.wav", music_title, MUSIC_PEAK),
    ("match_loop.wav", music_match, MUSIC_PEAK),
    ("heartbeat_loop.wav", cue_heartbeat_loop, MUSIC_PEAK),
]


def main() -> None:
    os.makedirs(OUT_DIR, exist_ok=True)
    print(f"Generating {len(CUES)} cues → {os.path.relpath(OUT_DIR, ROOT)}/")
    for name, fn, peak in CUES:
        # Name-derived deterministic stream: stable under insertion/reorder.
        h = 0
        for ch in name:
            h = (h * 131 + ord(ch)) & 0xFFFFFFFF
        rng = random.Random(SEED ^ h)
        sig = normalize(fn(rng), peak)
        write_wav(name, sig)
    print("done.")


if __name__ == "__main__":
    main()
