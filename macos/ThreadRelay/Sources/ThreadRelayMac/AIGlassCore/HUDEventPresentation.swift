import SwiftUI

/// Dashboard 记录列表使用的事件展示元数据。
extension HUDEvent.Kind {
    var iconName: String {
        switch self {
        case .limitThreshold: "exclamationmark.triangle"
        case .depletionRisk: "hourglass"
        case .windowReset: "arrow.clockwise"
        case .burnSpike: "flame"
        case .briefing: "sun.max"
        case .comeback: "arrow.uturn.forward"
        case .milestone: "sparkles"
        case .record: "trophy"
        case .update: "arrow.down.circle"
        }
    }

    var iconColor: Color {
        switch self {
        case .limitThreshold, .depletionRisk: .orange
        case .windowReset, .comeback, .briefing: .blue
        case .burnSpike: .red
        case .milestone, .record: .green
        case .update: .secondary
        }
    }
}
