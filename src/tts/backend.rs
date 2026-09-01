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

#[allow(async_fn_in_trait)]
pub trait TtsBackend {
    async fn synth(&self, text: &str) -> anyhow::Result<Synthesized>;
}
