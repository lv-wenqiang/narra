"""合成两段音效，纯 Python 标准库，确定性（固定种子）。

产出 44.1kHz 单声道 16-bit WAV，再由 ffmpeg 转 mp3。
自制素材，无第三方授权牵扯。
"""
import math, random, struct, wave

SR = 44100


def env_exp(n, tau, attack=0.002):
    """指数衰减包络，带极短攻击段（避免咔哒断音）。"""
    a = max(1, int(attack * SR))
    out = []
    for i in range(n):
        amp = math.exp(-i / (tau * SR))
        if i < a:
            amp *= i / a
        out.append(amp)
    return out


def keystrike(rng):
    """一次击键：高频瞬态 + 中频共鸣 + 低频闷响。"""
    dur = 0.075
    n = int(dur * SR)
    # 瞬态：白噪声，极快衰减
    trans = env_exp(n, 0.006)
    noise = [(rng.random() * 2 - 1) for _ in range(n)]
    # 简单一阶高通，让瞬态更脆
    hp, prev_in, prev_out = [], 0.0, 0.0
    alpha = 0.72
    for x in noise:
        y = alpha * (prev_out + x - prev_in)
        hp.append(y)
        prev_in, prev_out = x, y
    # 中频共鸣：两条略微失谐的衰减正弦
    f1 = rng.uniform(1750, 2450)
    f2 = f1 * rng.uniform(1.42, 1.63)
    res = env_exp(n, 0.018)
    # 低频闷响：机械触底
    f3 = rng.uniform(135, 175)
    low = env_exp(n, 0.024)

    out = []
    for i in range(n):
        t = i / SR
        s = hp[i] * trans[i] * 0.55
        s += math.sin(2 * math.pi * f1 * t) * res[i] * 0.20
        s += math.sin(2 * math.pi * f2 * t) * res[i] * 0.12
        s += math.sin(2 * math.pi * f3 * t) * low[i] * 0.30
        out.append(s)
    return out


def typewriter(total_sec, seed=20260905):
    rng = random.Random(seed)
    n = int(total_sec * SR)
    buf = [0.0] * n
    t = 0.04
    while t < total_sec - 0.09:
        k = keystrike(rng)
        off = int(t * SR)
        gain = rng.uniform(0.78, 1.0)
        for i, s in enumerate(k):
            if off + i < n:
                buf[off + i] += s * gain
        # 打字节奏：多数间隔较短，偶尔一个较长的停顿
        gap = rng.uniform(0.095, 0.155)
        if rng.random() < 0.13:
            gap += rng.uniform(0.10, 0.22)
        t += gap
    return buf


def bell(total_sec):
    """片尾钟声：类管钟的非谐泛音，高次泛音衰减更快。"""
    n = int(total_sec * SR)
    f0 = 587.33  # D5
    # (频率比, 振幅, 衰减时间常数)
    parts = [
        (1.000, 1.00, 0.90),
        (2.000, 0.52, 0.62),
        (2.760, 0.34, 0.42),
        (4.070, 0.20, 0.28),
        (5.400, 0.13, 0.20),
        (8.930, 0.07, 0.13),
    ]
    envs = [(r, a, env_exp(n, tau, attack=0.006)) for r, a, tau in parts]
    buf = []
    for i in range(n):
        t = i / SR
        s = 0.0
        for r, a, e in envs:
            s += math.sin(2 * math.pi * f0 * r * t) * a * e[i]
        buf.append(s)
    return buf


def write_wav(path, buf, headroom=0.89):
    peak = max(abs(x) for x in buf) or 1.0
    g = headroom / peak
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(b"".join(struct.pack("<h", int(max(-1, min(1, x * g)) * 32767)) for x in buf))
    rms = math.sqrt(sum((x * g) ** 2 for x in buf) / len(buf))
    print(f"  {path}: {len(buf)/SR:.3f}s  峰值归一至 {headroom}  RMS {20*math.log10(rms):.1f} dBFS")


write_wav("typewriter.wav", typewriter(3.157))
write_wav("bell.wav", bell(2.486))
