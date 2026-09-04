# panda-video-rs 的任务入口。
#
# 这里只放**编排**——把已有的 panda 子命令按顺序串起来，不实现任何逻辑。
# 曾经有一个 `panda make` 子命令干这件事，它在二进制里是一层纯胶水（跑 TTS、
# 拼出产物路径、转手调用合成），既没有可注入的接缝、也没有一条测试覆盖
# （见 docs/follow-ups.md 已销账的「族 B」条目）。编排移到这里之后，那层
# 胶水不复存在，`panda` 只剩三个各自可测的子命令。

# 与 src/config.rs 的默认值镜像。**改这里必须同步改那边**——
# tests/justfile_defaults.rs 会逐条比对，改一边不改另一边会让它变红。
tts_outdir := env_var_or_default("TTS_OUTPUT_DIR", "output/tts")
bg := env_var_or_default("BG_VIDEO", "public/video/0.mp4")
bgm := env_var_or_default("BGM_FILE", "public/bgm/0.mp3")

# 列出所有配方
default:
    @just --list

# `make` 配方的两点说明：
#
# 1. 素材的存在性**在跑 TTS 之前**就检查（销 docs/follow-ups.md 的「素材校验
#    晚于 TTS」条目）：`--bg` 打错一个字，此前要先付一整轮 Edge TTS 网络往返
#    （实测约 10s，长文稿更久）才报错。TTS 是这条链上唯一要联网、唯一耗时以
#    分钟计的一步，任何能在它之前发现的错误都应该在它之前发现。
# 2. 额外参数原样透传给 `panda render`，例如：
#      just make 文稿.txt --brand 墨风 --watermark-cover "墨风 · 自动化引擎"

# 文稿 → TTS → 成片，一条龙
make input="" *render_args="":
    #!/usr/bin/env bash
    set -euo pipefail

    for pair in "背景视频:{{ bg }}" "背景音乐:{{ bgm }}"; do
        label="${pair%%:*}"; path="${pair#*:}"
        if [ ! -f "$path" ]; then
            echo "${label}文件不存在：${path}" >&2
            echo "（在跑 TTS 之前就检查，免得白付一轮网络往返）" >&2
            exit 1
        fi
    done

    if [ -n "{{ input }}" ]; then
        cargo run --release --quiet -- tts "{{ input }}"
    else
        cargo run --release --quiet -- tts
    fi

    cargo run --release --quiet -- render \
        --audio "{{ tts_outdir }}/audio.mp3" \
        --vtt "{{ tts_outdir }}/audio.vtt" \
        --bg "{{ bg }}" \
        --bgm "{{ bgm }}" \
        {{ render_args }}
