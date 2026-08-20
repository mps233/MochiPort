import AppKit

/// 알림성 이벤트 시 시스템 사운드를 재생한다.
/// settings.funSoundEnabled 가드는 호출부(App)에서. NSSound는 메인 스레드 전용.
@MainActor
enum SoundPlayer {
    /// 시스템 사운드 "Glass"를 한 번 재생한다. 없으면 무시.
    static func play() {
        NSSound(named: "Glass")?.play()
    }
}
