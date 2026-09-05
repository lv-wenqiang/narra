# narra 的任务入口。
#
# 这里只放**编排**——把已有的 narra 子命令按顺序串起来，不实现任何逻辑。
# 曾经有一个 `narra make` 子命令干这件事，它在二进制里是一层纯胶水（跑 TTS、
# 拼出产物路径、转手调用合成），既没有可注入的接缝、也没有一条测试覆盖
# （见 docs/follow-ups.md 已销账的「族 B」条目）。编排移到这里之后，那层
# 胶水不复存在，`narra` 只剩三个各自可测的子命令。

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
# 2. 额外参数原样透传给 `narra render`，例如：
#      just make 文稿.txt --brand 墨 --watermark-cover "墨 · 自动化引擎"
#      just make 文稿.txt --orientation portrait

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

# `make-both` 把同一次 TTS 的产物渲染两遍——TTS 是整条链上唯一要联网、唯一
# 耗时以分钟计的一步，两档成片没有理由各付一次。
#
# **`render_args` 里不要传 `--orientation`**：本配方已经给两条腿各自加了一个
# （横版一次、竖版一次），再透传一个就是同一个 flag 出现两次，clap 会在**第
# 一条腿**上直接报错退出，`set -euo pipefail` 让整个配方在那里停住——也就是
# 白跑一轮 TTS 才失败。要单出一档请用 `just make … --orientation portrait`。

# 一份文稿出两档成片（横版 + 竖版），复用同一次 TTS
make-both input="" *render_args="":
    #!/usr/bin/env bash
    set -euo pipefail
    just make "{{ input }}" --orientation landscape -o output/video/landscape.mp4 {{ render_args }}
    # 第二次跳过 TTS：产物已在 {{ tts_outdir }}，直接渲染
    cargo run --release --quiet -- render \
        --audio "{{ tts_outdir }}/audio.mp3" \
        --vtt "{{ tts_outdir }}/audio.vtt" \
        --bg "{{ bg }}" --bgm "{{ bgm }}" \
        --orientation portrait -o output/video/portrait.mp4 {{ render_args }}
