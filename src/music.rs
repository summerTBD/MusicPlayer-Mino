//! 音乐文件扫描与识别

use std::fs;
use std::path::{Path, PathBuf};

/// 支持的音频扩展名（与 rodio 默认解码能力对应）
const SUPPORTED_EXTENSIONS: [&str; 7] = ["mp3", "m4a", "aac", "flac", "wav", "ogg", "mp4"];

/// 判断路径是否为受支持的音频文件
pub fn is_music_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|s| ext.eq_ignore_ascii_case(s))
        })
        .unwrap_or(false)
}

/// 扫描目录下所有音频文件，并按文件名排序
pub fn collect_music_files(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut list = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && is_music_file(&path) {
            list.push(path);
        }
    }
    list.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(list)
}
