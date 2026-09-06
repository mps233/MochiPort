import AppKit

/// 通知类事件触发时播放系统音效。
/// settings.funSoundEnabled 的开关检查在调用方（App）。NSSound 仅限主线程。
@MainActor
enum SoundPlayer {
    /// 播放一次系统音效 "Glass"。不存在则忽略。
    static func play() {
        NSSound(named: "Glass")?.play()
    }
}
