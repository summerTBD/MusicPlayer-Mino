//! 终端音乐播放器（二进制入口）
//!
//! 所有逻辑都在库 crate 中（见 `lib.rs` 及其子模块），这里只做薄封装。

fn main() -> Result<(), Box<dyn std::error::Error>> {
    terminal_music_player::run()
}
