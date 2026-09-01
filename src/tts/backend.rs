#[derive(Debug, Clone)]
pub struct WordTiming {
    pub text: String,
    pub offset_ms: u64,
    pub duration_ms: u64,
}

#[derive(Debug)]
pub struct Synthesized {
    pub audio: Vec<u8>,
    /// Edge 的 WordBoundary 事件。首版收集但不使用，为后续精确字幕对齐预留。
    pub timings: Option<Vec<WordTiming>>,
}

/// `synth` 的返回类型显式声明为 `impl Future<...> + Send`（而非裸的 `async fn`
/// 搭配 `#[allow(async_fn_in_trait)]`），使得 trait 对外暴露一个可被泛型代码
/// 约束 `Send` 的 Future。这是流水线（`src/tts/pipeline.rs`）能用
/// `tokio::spawn`（而非需要 `LocalSet` 的 `spawn_local`）调度合成任务的前提。
/// `EdgeBackend::synth` 本身返回的 Future 一直就是 `Send` 的，这个 bound 对
/// 真实后端零成本。
pub trait TtsBackend {
    fn synth(
        &self,
        text: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<Synthesized>> + Send;
}
