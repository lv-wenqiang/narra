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

/// 阻尼谐振子的阶跃响应，从 0 平滑趋近 1。
///
/// 参数为规格 §8.5 规定的 `mass=1, stiffness=100, damping=200`，
/// 阻尼比 `zeta = 10`，属**过阻尼**，因此曲线单调无过冲。
///
/// Remotion 传入 `durationInFrames` 时会把曲线在时间轴上重新缩放。
/// 这里用**归一化**达到同一意图：把 `t` 映射到 `[0, 1]` 的归一化时间后求值，
/// 再除以 `t=1` 处的值，保证 `spring(0)=0`、`spring(duration)=1` 精确成立。
/// 数值上未必与 Remotion 逐帧一致，但本项目不要求逐像素对拍（规格 §10）。
pub fn spring(frame: f64, fps: f64, duration_frames: f64, delay_frames: f64) -> f64 {
    let elapsed = frame - delay_frames;
    if elapsed <= 0.0 || duration_frames <= 0.0 {
        return 0.0;
    }
    if elapsed >= duration_frames {
        return 1.0;
    }

    let w0 = (SPRING_STIFFNESS / SPRING_MASS).sqrt();
    let zeta = SPRING_DAMPING / (2.0 * (SPRING_STIFFNESS * SPRING_MASS).sqrt());

    // 归一化时间：把 duration_frames 映射成 1 秒的自然时长
    let natural_duration_secs = duration_frames / fps;
    let step = |u: f64| -> f64 {
        let t = u * natural_duration_secs;
        if zeta > 1.0 {
            let wd = w0 * (zeta * zeta - 1.0).sqrt();
            1.0 - (-zeta * w0 * t).exp() * ((wd * t).cosh() + (zeta * w0 / wd) * (wd * t).sinh())
        } else if (zeta - 1.0).abs() < 1e-12 {
            1.0 - (-w0 * t).exp() * (1.0 + w0 * t)
        } else {
            let wd = w0 * (1.0 - zeta * zeta).sqrt();
            1.0 - (-zeta * w0 * t).exp() * ((wd * t).cos() + (zeta * w0 / wd) * (wd * t).sin())
        }
    };

    let u = elapsed / duration_frames;
    let denom = step(1.0);
    if denom.abs() < 1e-12 {
        // 极端参数下退化为线性，避免除零
        return u;
    }
    (step(u) / denom).clamp(0.0, 1.0)
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
    fn spring_respects_delay() {
        // delay 30 帧：之前恒为 0
        assert_eq!(spring(0.0, 30.0, 15.0, 30.0), 0.0);
        assert_eq!(spring(29.0, 30.0, 15.0, 30.0), 0.0);
        assert_eq!(spring(30.0, 30.0, 15.0, 30.0), 0.0);
        assert!(spring(38.0, 30.0, 15.0, 30.0) > 0.0);
        assert!((spring(45.0, 30.0, 15.0, 30.0) - 1.0).abs() < 1e-9);
    }
}
