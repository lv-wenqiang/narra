//! 帧流的两段流水线：渲染在一头，写出在另一头，中间挂一个有界通道。

use anyhow::{Context, Result};
use std::io::Write;

/// 通道容量：写端最多能落后生产端几帧。
///
/// 内存代价是 `(CAPACITY + 1) × 一帧字节数`（通道里 `CAPACITY` 帧，生产者手上
/// 还有 1 帧）——LANDSCAPE 一帧 8.3MB，取 2 就是约 25MB。再大没有意义：这条
/// 流水线只有两段，容量只要能盖住两端的**抖动**即可，盖不住的是两端的**均速差**，
/// 那是另一回事（见 `docs/ffmpeg-pipeline.md` §11）。
pub const CAPACITY: usize = 2;

/// 逐帧生产、并发写出，返回写端实际写出的帧数。
///
/// **为什么要拆两段**（`docs/ffmpeg-pipeline.md` §11 实测）：写端与 ffmpeg 之间
/// 的管道缓冲区只有 64KB，是一帧（1920×1080×4 = 8.3MB）的 1/127。生产与写出
/// 在同一个线程上时，`produce → write` 严格轮流：`write` 会一直卡到 ffmpeg 把
/// 这一帧吃完，期间下一帧连渲染都没开始。实测端到端 20.66s 落在
/// 「完全重叠 16.94s」与「完全串行 24.76s」之间、偏串行那头，白丢约 18%。
///
/// `produce` 留在**调用线程**（它要 `&mut FrameSource`，不必是 `Send`），
/// 写出挪到作用域线程。两条错误路径的取舍：
///
/// - **写端失败**（ffmpeg 提前退出 → broken pipe）：通道随写端一起断开，
///   生产端的 `send` 立刻失败并停下，**不会把剩下的帧白渲染完**；错误以写端的
///   为准报出来，因为它才是根因。
/// - **生产端失败**：停止发送、`drop` 通道让写端自然收尾，再把错误报出去。
pub fn stream_frames<P, W>(
    total: u32,
    frame_bytes: usize,
    capacity: usize,
    mut produce: P,
    out: &mut W,
) -> Result<u32>
where
    P: FnMut(u32, &mut Vec<u8>) -> Result<()>,
    W: Write + Send,
{
    // 帧通道是**有界**的：写端落后超过 `capacity` 帧，生产端就会在 `send`
    // 上等——内存占用因此有上限，不会因为写端慢就把整条时间轴堆进内存。
    let (frame_tx, frame_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(capacity);

    // 缓冲区在两端之间**循环复用**，不是每帧新分配：一帧 8.3MB，新分配意味着
    // 每帧多 2048 个缺页中断（实测量级约 1ms/帧，789 帧就是近 1 秒）。池子里
    // 一共 `capacity + 1` 个（通道里最多 `capacity` 个 + 生产端手上 1 个）。
    //
    // `pool_tx` 交给写端持有：写端一旦结束（出错或收工），它随之析构，生产端
    // 的 `pool_rx.recv()` 立刻返回 `Err` 而不是永久等一个再也回不来的缓冲区。
    let (pool_tx, pool_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    for _ in 0..=capacity {
        pool_tx
            .send(Vec::with_capacity(frame_bytes))
            .expect("接收端就在本函数里，此刻不可能已断开");
    }

    std::thread::scope(|scope| {
        let writer = scope.spawn(move || -> Result<u32> {
            let mut n = 0u32;
            while let Ok(buf) = frame_rx.recv() {
                out.write_all(&buf)
                    .with_context(|| format!("写第 {n} 帧到管道失败"))?;
                n += 1;
                // 还不回去也只是少一个复用缓冲区，不是错误：生产端可能已经走了。
                let _ = pool_tx.send(buf);
            }
            out.flush().context("刷新帧流管道失败")?;
            Ok(n)
        });

        let mut produce_err = None;
        for f in 0..total {
            // 两处 `break` 都是「写端没了」：错误由下面 join 出来的写端结果
            // 报出，这里不猜原因。
            let Ok(mut buf) = pool_rx.recv() else { break };
            if let Err(e) = produce(f, &mut buf) {
                produce_err = Some(e);
                break;
            }
            // 写端已经走了。这一行是**快路径**，不是唯一防线：即使去掉它，
            // 下一轮 `pool_rx.recv()` 也会失败（`pool_tx` 随写端一起析构）。
            // 差别只在多白渲染几帧——变异验证确认
            // `a_failing_writer_stops_the_producer_...` 区分不出这两种写法，
            // 留着它是为了让「写端没了就停」这个意图写在它该在的位置上。
            if frame_tx.send(buf).is_err() {
                break;
            }
        }
        // 关掉帧通道，写端的 `recv` 才会结束——与 `run_render` 里 `drop(stdin)`
        // 是同一件事的两层：这里让写端收工，那里让 ffmpeg 收工。
        drop(frame_tx);

        let written = match writer.join() {
            Ok(r) => r,
            Err(panic) => std::panic::resume_unwind(panic),
        };

        // **写端的错误优先**：ffmpeg 提前退出时，生产端看到的只是「通道断了」
        // 这个后果，写端手里的 broken pipe 才是根因。与 `run_render` 里
        // 「ffmpeg 的 stderr 优先于写帧错误」同一个取舍。
        let written = written?;
        match produce_err {
            Some(e) => Err(e),
            None => Ok(written),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 每帧只写 4 个字节：帧号的小端表示。够用来验顺序，又不必搬 8MB。
    const TINY_FRAME: usize = 4;

    fn produce_index(f: u32, buf: &mut Vec<u8>) -> Result<()> {
        buf.clear();
        buf.extend_from_slice(&f.to_le_bytes());
        Ok(())
    }

    /// 写端：第一次写之前先等生产端跑到第 `gate_at` 帧，等不到就报错。
    ///
    /// **这是本模块存在理由的判据**：串行实现下，生产端必须等这次 `write`
    /// 返回才会渲染下一帧，而这次 `write` 又在等生产端往前跑——它永远等不到，
    /// `DEADLINE` 到点后返回 `Err`。流水线实现下，生产端手上还有一帧、通道里
    /// 还能放 `capacity` 帧，闸门瞬间就开。**不用 sleep 制造时序**：判据是
    /// 「等到 / 等不到」，不是「谁快谁慢」。
    struct GatedWriter {
        produced: Arc<AtomicU32>,
        gate_at: u32,
        opened: bool,
        got: Vec<u32>,
    }

    /// 闸门等待上限。通过路径下微秒级就开，只有失败路径才会吃满。
    const DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

    impl Write for GatedWriter {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if !self.opened {
                let t0 = std::time::Instant::now();
                while self.produced.load(Ordering::SeqCst) < self.gate_at {
                    if t0.elapsed() > DEADLINE {
                        return Err(std::io::Error::other(format!(
                            "写端等了 {}s，生产端仍停在第 {} 帧——说明两段是串行的：\
                             生产端在等这次 write 返回，而这次 write 在等生产端往前跑",
                            DEADLINE.as_secs(),
                            self.produced.load(Ordering::SeqCst)
                        )));
                    }
                    std::thread::yield_now();
                }
                self.opened = true;
            }
            self.got
                .push(u32::from_le_bytes(b[..4].try_into().unwrap()));
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn producer_runs_ahead_while_the_writer_is_still_on_the_first_frame() {
        let produced = Arc::new(AtomicU32::new(0));
        let p = produced.clone();
        let mut w = GatedWriter {
            produced: produced.clone(),
            // 通道容量 2 → 生产端手上 1 帧 + 通道里 2 帧 = 能跑到第 3 帧
            gate_at: 3,
            opened: false,
            got: Vec::new(),
        };

        let n = stream_frames(
            8,
            TINY_FRAME,
            2,
            |f, buf| {
                produce_index(f, buf)?;
                p.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            &mut w,
        )
        .unwrap();

        assert_eq!(n, 8);
        assert_eq!(w.got, (0..8).collect::<Vec<_>>(), "帧序不能被并发打乱");
    }

    /// 顺序是硬要求：帧流是位置敏感的裸字节，错序等于画面错乱。
    #[test]
    fn frames_reach_the_writer_in_order() {
        let mut got: Vec<u32> = Vec::new();
        struct Collect<'a>(&'a mut Vec<u32>);
        impl Write for Collect<'_> {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.push(u32::from_le_bytes(b[..4].try_into().unwrap()));
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let n = stream_frames(
            64,
            TINY_FRAME,
            CAPACITY,
            produce_index,
            &mut Collect(&mut got),
        )
        .unwrap();
        assert_eq!(n, 64);
        assert_eq!(got, (0..64).collect::<Vec<_>>());
    }

    /// 写端失败之后不能继续白渲染：ffmpeg 提前退出时，剩下几百帧渲染出来
    /// 也只是喂给一根断掉的管子。
    #[test]
    fn a_failing_writer_stops_the_producer_instead_of_letting_it_render_on() {
        struct FailAt(u32, u32);
        impl Write for FailAt {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.1 += 1;
                if self.1 > self.0 {
                    return Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
                }
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let err = stream_frames(
            1000,
            TINY_FRAME,
            CAPACITY,
            |f, buf| {
                c.fetch_add(1, Ordering::SeqCst);
                produce_index(f, buf)
            },
            &mut FailAt(5, 0),
        )
        .expect_err("写端 broken pipe 应报错");

        let msg = format!("{err:#}");
        assert!(msg.contains("写第"), "错误应指出是写帧失败：{msg}");
        let n = calls.load(Ordering::SeqCst);
        assert!(
            n <= 5 + CAPACITY as u32 + 2,
            "写端第 6 次写就断了，生产端最多再多跑通道容量那几帧，实际跑了 {n} 帧"
        );
    }

    /// 生产端失败时，错误要原样报出来，不能被写端的收尾掩盖。
    #[test]
    fn a_failing_producer_reports_its_own_error() {
        let mut sink = std::io::sink();
        let err = stream_frames(
            100,
            TINY_FRAME,
            CAPACITY,
            |f, buf| {
                produce_index(f, buf)?;
                if f == 3 {
                    anyhow::bail!("第 3 帧渲染失败");
                }
                Ok(())
            },
            &mut sink,
        )
        .expect_err("生产端失败应报错");
        assert!(format!("{err:#}").contains("第 3 帧渲染失败"), "{err:#}");
    }
}
