import SwiftUI

enum Theme {
    static let safeGreen = Color(red: 0.35, green: 0.82, blue: 0.54)

    /// The quiet gray used by the light-mode "today's requests" card.
    static let usageLightSurface = Color.primary.opacity(0.055)

    /// Opaque equivalent of the heatmap's first populated gray level. It
    /// keeps chart grid lines from showing through while staying restrained.
    static let usageLightOpaqueGray = Color(red: 0.82, green: 0.82, blue: 0.82)

    /// Light-mode Mochi colors sampled from the user's blue mascot reference.
    static let mascotLightFill = Color(red: 0.808, green: 0.929, blue: 0.941)
    static let mascotLightInk = Color(red: 0.271, green: 0.271, blue: 0.271)

    static func color(for service: ServiceID) -> Color {
        let c = rgb(for: service)
        return Color(red: c.r, green: c.g, blue: c.b)
    }

    /// Codex 主题色的 RGB 成分（混合色计算用）。
    static func rgb(for service: ServiceID) -> (r: Double, g: Double, b: Double) {
        _ = service
        return (0.31, 0.79, 0.64) // Codex mint #4FC9A3
    }
    /// 사용률 상태 색. warn/crit은 설정 임계값 주입용 (기본 70/90 — 기존 호출부 호환).
    static func statusColor(percent: Double, warn: Double = 70, crit: Double = 90) -> Color {
        if percent >= crit { return .red }
        if percent >= warn { return .orange }
        return safeGreen
    }

    static func formatUsagePercent(_ percent: Double) -> String {
        if percent <= 0 { return "0%" }
        return "\(max(1, Int(percent.rounded())))%"
    }
}

struct GaugeBar: View {
    let percent: Double
    let tint: Color
    /// nil이면 기존처럼 즉시 표시. 값이 있으면 onAppear 시 0→값으로 슈욱 (delay초 뒤, 행 stagger용).
    var appearDelay: Double? = nil
    @State private var appeared = false

    private var displayPercent: Double {
        (appearDelay == nil || appeared) ? percent : 0
    }

    var body: some View {
        GeometryReader { geo in
            let p = displayPercent
            let fillWidth = p <= 0 ? 0 : max(4, geo.size.width * min(1, p / 100))
            ZStack(alignment: .leading) {
                Capsule().fill(.quaternary)
                Capsule()
                    .fill(tint.gradient)
                    .frame(width: fillWidth)
            }
        }
        .frame(height: 6)
        .animation(.spring(duration: 0.5), value: percent)
        .onAppear {
            guard let appearDelay else { return }
            withAnimation(.spring(duration: 0.7).delay(appearDelay)) { appeared = true }
        }
    }
}

/// A stable, muted track for a quota window whose metadata is unavailable.
struct DisabledGaugeBar: View {
    var body: some View {
        Capsule()
            .fill(.quaternary.opacity(0.65))
            .frame(maxWidth: .infinity)
            .frame(height: 6)
    }
}
