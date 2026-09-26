//! 全局配置：内置默认值 + 可选的 `mino.toml` 覆盖
//!
//! 优先级：`mino.toml` > [`Config::default`]。
//! 找不到文件就全用默认值；文件存在但写错时只警告并回退默认值
//! —— 配置写错不该让播放器起不来。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// 配置文件名（启动时按「当前目录 → 可执行文件所在目录」的顺序查找）
pub const CONFIG_FILE_NAME: &str = "mino.toml";

/// UI 刷新间隔（毫秒），同时用作按键轮询超时
pub const UI_REFRESH_MS: u64 = 100;

/// 每次按 `+` / `-` 调整音量的步长
pub const VOLUME_STEP: f32 = 0.05;

/// 运行期配置。所有字段都能在 `mino.toml` 里覆盖，缺哪项就用哪项的默认值。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 音乐目录
    pub music_directory: PathBuf,
    /// 启动时的初始音量 (0.0 ~ 1.0)；运行中可用 `+` / `-` 键继续调整
    pub volume: f32,

    /// 实际读到的配置文件；`None` = 没找到，全用默认值
    #[serde(skip)]
    pub source: Option<PathBuf>,
    /// 读取或解析失败的原因（界面顶部会红字提示）
    #[serde(skip)]
    pub error: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            music_directory: PathBuf::from(r"E:\MyMusic"),
            volume: 0.8,
            source: None,
            error: None,
        }
    }
}

impl Config {
    /// 按「当前目录 → 可执行文件所在目录」顺序取第一个存在的 [`CONFIG_FILE_NAME`]
    pub fn load() -> Self {
        let mut candidates = vec![PathBuf::from(CONFIG_FILE_NAME)];
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            candidates.push(dir.join(CONFIG_FILE_NAME));
        }

        for path in &candidates {
            match fs::read_to_string(path) {
                Ok(text) => return Self::parse(&text, path),
                // 这个位置没文件 → 看下一个候选路径
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                // 有文件但读不了（权限等）→ 保留原因给界面
                Err(e) => {
                    return Self {
                        error: Some(format!("读取 {} 失败: {e}", path.display())),
                        ..Self::default()
                    };
                }
            }
        }
        Self::default()
    }

    /// 解析文本；手写的值可能越界，就地夹紧，免得越界音量直达混音器
    fn parse(text: &str, path: &Path) -> Self {
        match toml::from_str::<Self>(text) {
            Ok(mut config) => {
                config.volume = config.volume.clamp(0.0, 1.0);
                config.source = Some(path.to_path_buf());
                config
            }
            Err(e) => Self {
                error: Some(format!("解析 {} 失败: {e}", path.display())),
                ..Self::default()
            },
        }
    }

    /// 界面上展示的配置来源说明
    pub fn source_label(&self) -> String {
        match (&self.source, &self.error) {
            (Some(path), None) => path.display().to_string(),
            (_, None) => format!("默认值（未找到 {CONFIG_FILE_NAME}）"),
            (_, Some(_)) => "默认值".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Config {
        Config::parse(text, Path::new(CONFIG_FILE_NAME))
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let config = parse("volume = 0.25\n");
        assert_eq!(config.volume, 0.25);
        // 没写的字段用默认值（与默认配置对照，免得把默认目录写死在测试里）
        assert_eq!(config.music_directory, Config::default().music_directory);
        assert!(config.error.is_none());
    }

    #[test]
    fn reads_windows_path_written_as_literal_string() {
        let config = parse(r"music_directory = 'D:\Music'");
        assert_eq!(config.music_directory, PathBuf::from(r"D:\Music"));
    }

    #[test]
    fn clamps_out_of_range_volume() {
        assert_eq!(parse("volume = 3.0\n").volume, 1.0);
        assert_eq!(parse("volume = -1.0\n").volume, 0.0);
    }

    #[test]
    fn bad_toml_falls_back_and_reports_error() {
        let config = parse("volume = [\n");
        assert!(config.error.is_some());
        assert_eq!(config.volume, Config::default().volume);
    }

    #[test]
    fn records_which_file_was_loaded() {
        assert_eq!(
            parse("volume = 0.5\n").source,
            Some(PathBuf::from(CONFIG_FILE_NAME))
        );
    }
}
