//! 播放器共享状态与命令定义

use std::path::PathBuf;
use std::time::Duration;

/// 播放模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayMode {
    /// 顺序播放（播完最后一首停止）
    Sequential,
    /// 列表循环
    LoopAll,
    /// 单曲循环
    LoopSingle,
}

impl PlayMode {
    /// 切换到下一个模式
    pub fn cycle(&self) -> Self {
        match self {
            PlayMode::Sequential => PlayMode::LoopAll,
            PlayMode::LoopAll => PlayMode::LoopSingle,
            PlayMode::LoopSingle => PlayMode::Sequential,
        }
    }

    pub fn to_display_str(&self) -> &'static str {
        match self {
            PlayMode::Sequential => "顺序播放",
            PlayMode::LoopAll => "列表循环",
            PlayMode::LoopSingle => "单曲循环",
        }
    }
}

/// 音频控制命令（主线程 -> 音频线程）
#[derive(Debug)]
pub enum Command {
    /// 播放指定索引（0-based）
    Play(usize),
    /// 下一首
    Next,
    /// 上一首
    Prev,
    /// 切换暂停/继续
    TogglePause,
    /// 切换播放模式
    SetMode(PlayMode),
    /// 停止播放
    Stop,
    /// 退出程序
    Quit,
}

/// 播放器全局状态（跨线程共享）
pub struct PlayerState {
    pub playlist: Vec<PathBuf>,
    pub current_index: Option<usize>,
    pub total_duration: Duration,
    /// 当前播放进度（取自播放器内部计时，暂停时自动冻结）
    pub estimated_position: Duration,
    pub is_paused: bool,
    pub play_mode: PlayMode,
}

impl PlayerState {
    pub fn new(playlist: Vec<PathBuf>) -> Self {
        Self {
            playlist,
            current_index: None,
            total_duration: Duration::ZERO,
            estimated_position: Duration::ZERO,
            is_paused: false,
            play_mode: PlayMode::Sequential,
        }
    }

    /// 获取当前歌曲文件名
    pub fn current_song_name(&self) -> &str {
        self.current_index
            .and_then(|i| self.playlist.get(i))
            .and_then(|p| p.file_name()?.to_str())
            .unwrap_or("无")
    }
}
