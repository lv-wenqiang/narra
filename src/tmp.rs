//! 临时路径的作用域守卫。
//!
//! **为什么需要它**：把「清理」写在控制流的某一行上，就永远存在「哪条路径漏了」
//! 这个问题。`docs/follow-ups.md`「族 A」记的三处同根因缺陷都是这一种——
//! 生产代码里 `create_dir_all` 与清理之间夹着两个 `?` 出口，测试里被测闭包
//! panic 之后尾部的 `remove_dir_all` 不可达。把清理挂到**作用域**上，出口有
//! 几个、走的是 `?` 还是 unwind，都不需要再数一遍。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// 一个在 `Drop` 时被删除的临时路径（文件或目录皆可）。
///
/// **删除失败一律忽略**：析构里没有地方报告错误，而临时文件删不掉不该掩盖
/// 真正的失败原因；`/tmp` 本身也由系统兜底清理。这与它替换掉的
/// `cleanup_tmp_and_propagate`（`remove_dir_all(..).ok()`）语义一致。
#[derive(Debug)]
pub struct TempPath(PathBuf);

impl TempPath {
    /// 接管一个路径：本值被丢弃时删掉它。路径此刻不必已经存在。
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    /// 建一个唯一的临时目录并接管它。
    ///
    /// 目录名带 pid **与**随机后缀：同一个进程里跑两次合成（并发的单测、
    /// 将来的批量合成）时，只带 pid 的目录名会被两次共用，先结束的那次会
    /// 删掉另一次仍在被 ffmpeg 读取的文件，表现为偶发失败。取名方式沿用
    /// `tts::pipeline` 里的 `unique_tmp_dir`。
    pub fn create_dir(prefix: &str) -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "{prefix}_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path)
            .with_context(|| format!("创建临时目录失败：{}", path.display()))?;
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        // 文件与目录各试一次：`remove_dir_all` 对文件报错、`remove_file`
        // 对目录报错，两个都试比先 `metadata()` 判类型少一次系统调用，
        // 也不用处理「判断与删除之间被别人换掉了」的竞态。
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_removes_the_directory_and_everything_in_it() {
        let path = {
            let tmp = TempPath::create_dir("narra_tmp_test").unwrap();
            std::fs::write(tmp.path().join("a.txt"), b"x").unwrap();
            std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
            std::fs::write(tmp.path().join("sub/b.txt"), b"y").unwrap();
            assert!(tmp.path().exists(), "作用域内应存在");
            tmp.path().to_path_buf()
        };
        assert!(!path.exists(), "离开作用域后整棵目录都应被删掉：{path:?}");
    }

    #[test]
    fn dropping_removes_a_plain_file_too() {
        let path = std::env::temp_dir().join(format!("narra_tmp_file_{}", std::process::id()));
        std::fs::write(&path, b"x").unwrap();
        {
            let _guard = TempPath::new(path.clone());
            assert!(path.exists());
        }
        assert!(!path.exists(), "文件同样应被删掉：{path:?}");
    }

    /// **这条才是引入守卫的理由**：panic 展开时清理照样发生。
    ///
    /// 「族 A」的三处缺陷里有两处正是这个形状——被测闭包 panic 之后，写在
    /// 函数尾部的 `remove_dir_all` 永远执行不到。把清理挂在作用域上之后，
    /// 无论是 `?` 提前返回还是 unwind，都由 `Drop` 兜住。
    #[test]
    fn dropping_happens_even_when_the_scope_unwinds() {
        let path = std::env::temp_dir().join(format!("narra_tmp_panic_{}", std::process::id()));
        let p = path.clone();
        let result = std::panic::catch_unwind(move || {
            let _guard = TempPath::new(p.clone());
            std::fs::create_dir_all(&p).unwrap();
            assert!(p.exists());
            panic!("故意 panic，模拟被测闭包炸掉");
        });
        assert!(result.is_err(), "闭包应当确实 panic 了");
        assert!(!path.exists(), "panic 展开后临时路径也应被删掉：{path:?}");
    }

    /// 同一进程内两次 `create_dir` 必须给出不同目录。
    ///
    /// 裁定 R-T8-4 的由来：合成路径此前用只带 pid 的目录名，同进程里跑两次
    /// 合成（并发单测、将来的批量合成）会共用一个目录，先结束的那次清理会
    /// 删掉另一次仍被 ffmpeg 读取的内嵌音效，表现为偶发失败。
    #[test]
    fn create_dir_gives_a_distinct_directory_each_time() {
        let a = TempPath::create_dir("narra_render").unwrap();
        let b = TempPath::create_dir("narra_render").unwrap();
        assert_ne!(a.path(), b.path(), "同一进程内两次调用必须给出不同目录");
        assert_eq!(a.path().parent(), Some(std::env::temp_dir().as_path()));
        assert!(
            a.path()
                .file_name()
                .and_then(|f| f.to_str())
                .is_some_and(|n| n.starts_with("narra_render_")),
            "目录名应保留前缀便于人工辨认：{:?}",
            a.path()
        );
        assert!(a.path().exists() && b.path().exists(), "两个目录都应已创建");
    }

    /// 路径不存在时 `Drop` 不 panic——接管一个「将要创建」的路径是合法用法。
    #[test]
    fn dropping_a_never_created_path_is_harmless() {
        let path = std::env::temp_dir().join("narra_tmp_never_created_xyz");
        drop(TempPath::new(path.clone()));
        assert!(!path.exists());
    }
}
