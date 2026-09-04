/// 两点线性插值，输入超出区间时钳制到端点。
/// 对应 Remotion 的 `interpolate(x, input, output, {extrapolate*: 'clamp'})`。
pub fn interpolate(x: f64, input_range: [f64; 2], output_range: [f64; 2]) -> f64 {
    let [i0, i1] = input_range;
    let [o0, o1] = output_range;
    if x <= i0 {
        return o0;
    }
    if x >= i1 {
        return o1;
    }
    let t = (x - i0) / (i1 - i0);
    o0 + (o1 - o0) * t
}

/// 三点分段线性插值，两端钳制。用于光标闪烁这类「保持后再下降」的曲线。
pub fn interpolate3(x: f64, input_range: [f64; 3], output_range: [f64; 3]) -> f64 {
    let [i0, i1, i2] = input_range;
    let [o0, o1, o2] = output_range;
    if x <= i1 {
        interpolate(x, [i0, i1], [o0, o1])
    } else {
        interpolate(x, [i1, i2], [o1, o2])
    }
}

// 规格 §8.5：mass = 1, stiffness = 100, damping = 200
const SPRING_MASS: f64 = 1.0;
const SPRING_STIFFNESS: f64 = 100.0;
const SPRING_DAMPING: f64 = 200.0;
/// Remotion 的 `durationRestThreshold` 默认值：距终值小于此即视为静止。
const SPRING_REST_THRESHOLD: f64 = 0.005;

/// 阻尼谐振子的阶跃响应，从 0 平滑趋近 1。
///
/// 规格 §8.5 的参数 `mass=1, stiffness=100, damping=200` give 阻尼比 `zeta = 10`，
/// 属**过阻尼**，因此曲线单调、无过冲。
///
/// **时间映射**：把弹簧的**自然稳定时间**（本参数下约 10.6 秒 ≈ 317 帧 @30fps）
/// 压缩进 `duration_frames`，使整条曲线连同末尾的缓出都落在给定帧数内。
/// 这与 Remotion 传 `durationInFrames` 时的重缩放语义一致。
///
/// **不要**改成「把 `duration_frames / fps` 当成时间跨度直接映射」——本参数下
/// 那样只会取到自然曲线最前面约 22% 的一段，归一化后是近似匀速直线
/// （首末增量比约 1.07），完全失去弹簧观感。`spring_eases_out_rather_than_moving_linearly`
/// 这条测试就是钉住这一点的。
pub fn spring(frame: f64, fps: f64, duration_frames: f64, delay_frames: f64) -> f64 {
    let _ = fps; // 时间映射基于自然稳定时间，与 fps 无关；保留参数以维持调用形态
    let elapsed = frame - delay_frames;
    if elapsed <= 0.0 || duration_frames <= 0.0 {
        return 0.0;
    }
    if elapsed >= duration_frames {
        return 1.0;
    }

    let w0 = (SPRING_STIFFNESS / SPRING_MASS).sqrt();
    let zeta = SPRING_DAMPING / (2.0 * (SPRING_STIFFNESS * SPRING_MASS).sqrt());
    debug_assert!(zeta > 1.0, "本实现只覆盖过阻尼情形，当前 zeta = {zeta}");

    // 过阻尼解析解写成两个**衰减**指数之和。
    // 不用 cosh/sinh：它们在自然稳定时间那个量级（t ≈ 10.6s）会直接溢出。
    let wd = w0 * (zeta * zeta - 1.0).sqrt();
    let k = zeta * w0 / wd;
    let slow = zeta * w0 - wd;
    let fast = zeta * w0 + wd;
    let ca = (1.0 + k) / 2.0;
    let cb = (1.0 - k) / 2.0;
    let step = |t: f64| 1.0 - (ca * (-slow * t).exp() + cb * (-fast * t).exp());

    // step(t) = 1 - threshold 的时刻（快极点项在此量级已可忽略）
    let settle_secs = (ca / SPRING_REST_THRESHOLD).ln() / slow;
    let denom = step(settle_secs);
    if denom.abs() < 1e-12 {
        return elapsed / duration_frames; // 极端参数下退化为线性，避免除零
    }
    (step((elapsed / duration_frames) * settle_secs) / denom).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolate_hits_endpoints_exactly() {
        assert_eq!(interpolate(0.0, [0.0, 10.0], [1.2, 1.0]), 1.2);
        assert_eq!(interpolate(10.0, [0.0, 10.0], [1.2, 1.0]), 1.0);
    }

    #[test]
    fn interpolate_is_linear_in_between() {
        assert!((interpolate(5.0, [0.0, 10.0], [0.0, 100.0]) - 50.0).abs() < 1e-9);
        assert!((interpolate(2.5, [0.0, 10.0], [100.0, 0.0]) - 75.0).abs() < 1e-9);
    }

    #[test]
    fn interpolate_clamps_outside_range() {
        assert_eq!(interpolate(-5.0, [0.0, 10.0], [0.0, 1.0]), 0.0);
        assert_eq!(interpolate(99.0, [0.0, 10.0], [0.0, 1.0]), 1.0);
    }

    #[test]
    fn interpolate3_handles_the_cursor_blink_shape() {
        // 光标闪烁：[0, 7.5, 15] -> [1, 1, 0]
        assert_eq!(interpolate3(0.0, [0.0, 7.5, 15.0], [1.0, 1.0, 0.0]), 1.0);
        assert_eq!(interpolate3(7.5, [0.0, 7.5, 15.0], [1.0, 1.0, 0.0]), 1.0);
        assert!((interpolate3(11.25, [0.0, 7.5, 15.0], [1.0, 1.0, 0.0]) - 0.5).abs() < 1e-9);
        assert_eq!(interpolate3(15.0, [0.0, 7.5, 15.0], [1.0, 1.0, 0.0]), 0.0);
    }

    #[test]
    fn spring_hits_both_endpoints_exactly() {
        assert_eq!(spring(0.0, 30.0, 15.0, 0.0), 0.0);
        assert!((spring(15.0, 30.0, 15.0, 0.0) - 1.0).abs() < 1e-9);
        assert!((spring(999.0, 30.0, 15.0, 0.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn spring_is_monotonic_and_finite() {
        let mut prev = -1.0;
        for f in 0..=15 {
            let v = spring(f as f64, 30.0, 15.0, 0.0);
            assert!(v.is_finite(), "frame {f} 得到非有限值 {v}");
            assert!(v >= prev - 1e-12, "frame {f} 回退：{prev} -> {v}");
            prev = v;
        }
    }

    #[test]
    fn spring_eases_out_rather_than_moving_linearly() {
        // 过阻尼弹簧必须「先快后慢」。若误把 duration 当成时间跨度直接映射，
        // 只会取到自然曲线最前面约 22% 的一段，归一化后是近似匀速直线
        // （首末增量比约 1.07），本测试会失败。
        let v: Vec<f64> = (0..=15)
            .map(|f| spring(f as f64, 30.0, 15.0, 0.0))
            .collect();
        assert!(v[7] > 0.85, "半程应已完成大部分行程，实得 {}", v[7]);
        let first = v[1] - v[0];
        let last = v[15] - v[14];
        assert!(
            first > last * 20.0,
            "首帧增量应远大于末帧（缓出特征），实得 {first} vs {last}"
        );
    }

    #[test]
    fn spring_stays_finite_over_long_durations() {
        // 自然稳定时间约 10.6 秒；若用 cosh/sinh 直接求值会在此量级溢出。
        for dur in [15.0, 120.0, 600.0] {
            for f in 0..=(dur as u32) {
                let v = spring(f as f64, 30.0, dur, 0.0);
                assert!(v.is_finite(), "duration={dur} frame={f} 得到非有限值 {v}");
                assert!(
                    (0.0..=1.0).contains(&v),
                    "duration={dur} frame={f} 越界 {v}"
                );
            }
        }
    }

    #[test]
    fn spring_respects_delay() {
        // delay 30 帧：之前恒为 0
        assert_eq!(spring(0.0, 30.0, 15.0, 30.0), 0.0);
        assert_eq!(spring(29.0, 30.0, 15.0, 30.0), 0.0);
        assert_eq!(spring(30.0, 30.0, 15.0, 30.0), 0.0);
        assert!(spring(38.0, 30.0, 15.0, 30.0) > 0.0);
        assert!((spring(45.0, 30.0, 15.0, 30.0) - 1.0).abs() < 1e-9);
    }
}
