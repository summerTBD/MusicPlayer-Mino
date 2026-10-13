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
    /// 设置音量（0.0 ~ 1.0）
    SetVolume(f32),
    /// 重新打开默认输出设备（耳机断电重连、切换输出后没声音时用）
    RefreshDevice,
    /// 音频流出错：由 cpal 的错误回调投递，不是主线程发的命令
    StreamError(String),
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
    /// 当前音量（0.0 ~ 1.0）；主线程按键改，音频线程据此设置播放器
    pub volume: f32,
    /// 最近一次音频流错误（如耳机断开导致端点消失）；`None` = 一切正常。
    /// 界面据此提示用户按 `r` 重连
    pub stream_error: Option<String>,
}

impl PlayerState {
    /// `volume` 为初始音量（来自配置），取值 0.0 ~ 1.0
    pub fn new(playlist: Vec<PathBuf>, volume: f32) -> Self {
        Self {
            playlist,
            current_index: None,
            total_duration: Duration::ZERO,
            estimated_position: Duration::ZERO,
            is_paused: false,
            play_mode: PlayMode::Sequential,
            volume: volume.clamp(0.0, 1.0),
            stream_error: None,
        }
    }

    /// 按步长调整音量，返回夹紧后的新值。
    ///
    /// 结果取整到 1%（0.05 的二进制表示不精确，不取整会累出 79.999% 这种显示）
    pub fn adjust_volume(&mut self, delta: f32) -> f32 {
        let next = (self.volume + delta).clamp(0.0, 1.0);
        self.volume = (next * 100.0).round() / 100.0;
        self.volume
    }

    /// 获取当前歌曲文件名
    pub fn current_song_name(&self) -> &str {
        self.current_index
            .and_then(|i| self.playlist.get(i))
            .and_then(|p| p.file_name()?.to_str())
            .unwrap_or("无")
    }
}
