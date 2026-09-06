import SwiftUI

/// GitHub 分布图样式的用量热力图。
///
/// 最近 15 周 × 7 天（周一至周日）网格。列 = 周（左旧右新），
/// 行 = 星期（上为周一）。格子浓度 = 当日 Token / 期间最大 Token，量化为 5 档。
/// 悬停时格子放大上浮，并在上方弹出"日期 + 当日 Token"标签；
/// `legendPlacement == .bottom` 时底部说明行同步显示悬停信息。
/// 今日格子用描边强调；出现时按周 stagger 逐列填充（一次性入场动画）。
struct HeatmapView: View {
    enum LegendPlacement {
        case bottom
        case none
    }

    let statsStore: DailyStatsStore
    let enabledServices: Set<ServiceID>
    let cellSize: CGFloat
    let cellSpacing: CGFloat
    let legendPlacement: LegendPlacement

    private static let minimumWeeks = 15
    private static let maximumWeeks = 53
    private static let weekdayAxisWidth: CGFloat = 26
    private static let axisGap: CGFloat = 8
    private static let monthAxisHeight: CGFloat = 16
    private static let contentSpacing: CGFloat = 7

    init(
        statsStore: DailyStatsStore,
        enabledServices: Set<ServiceID>,
        cellSize: CGFloat = 9,
        cellSpacing: CGFloat = 3,
        legendPlacement: LegendPlacement = .bottom
    ) {
        self.statsStore = statsStore
        self.enabledServices = enabledServices
        self.cellSize = cellSize
        self.cellSpacing = cellSpacing
        self.legendPlacement = legendPlacement
    }

    @State private var appeared = false
    @State private var hovered: GridDay? = nil

    // 网格的一格 = 一个日期（或空位）。
    private struct GridDay: Identifiable, Equatable {
        let day: Date
        let tokens: Int
        var id: TimeInterval { day.timeIntervalSinceReferenceDate }
    }

    // 网格的列（周）——每周周一至周日 7 格。未来日期留空（nil）。
    private struct Week: Identifiable {
        let index: Int          // 0 = 最早的周，最后一列 = 当前周（stagger 顺序）
        let days: [GridDay?]    // 7 格（周一至周日）
        var id: Int { index }
    }

    private struct Layout {
        let weekCount: Int
        let cellSize: CGFloat
    }

    private var calendar: Calendar {
        // 日数据以 UTC 午夜的 Date 为键（与 30 天趋势一致），因此网格也按 UTC 构建。
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "UTC")!
        cal.firstWeekday = 2  // 周一开头
        return cal
    }

    // 每日 Token 字典（UTC 午夜 Date 为键）。
    private func dailyTokens(weeks: Int) -> [Date: Int] {
        let rows = statsStore.dailyTotals(days: weeks * 7, now: Date(),
                                          calendar: calendar, services: enabledServices)
        var dict: [Date: Int] = [:]
        for r in rows { dict[calendar.startOfDay(for: r.day)] = r.tokens }
        return dict
    }

    private var todayStart: Date { calendar.startOfDay(for: Date()) }

    // 让最右列（最新）落在当前周：从本周周一开始，向回取 (weekCount-1) 周。
    private func grid(weeks weekCount: Int) -> [Week] {
        let tokens = dailyTokens(weeks: weekCount)
        let today = todayStart
        let weekday = calendar.component(.weekday, from: today)  // 1=周日 … 2=周一
        let daysSinceMonday = (weekday + 5) % 7                  // 周一=0, 周日=6
        guard let thisMonday = calendar.date(byAdding: .day, value: -daysSinceMonday, to: today),
              let firstMonday = calendar.date(byAdding: .day, value: -(weekCount - 1) * 7, to: thisMonday)
        else { return [] }

        var weeks: [Week] = []
        for w in 0..<weekCount {
            guard let weekStart = calendar.date(byAdding: .day, value: w * 7, to: firstMonday) else { continue }
            var days: [GridDay?] = []
            for d in 0..<7 {
                guard let cellDay = calendar.date(byAdding: .day, value: d, to: weekStart) else {
                    days.append(nil); continue
                }
                if cellDay > today {
                    days.append(nil)  // 未来日期留空
                } else {
                    days.append(GridDay(day: cellDay, tokens: tokens[cellDay] ?? 0))
                }
            }
            weeks.append(Week(index: w, days: days))
        }
        return weeks
    }

    private func maxTokens(in weeks: [Week]) -> Int {
        max(1, weeks.flatMap { $0.days.compactMap { $0?.tokens } }.max() ?? 0)
    }

    // 5 档量化：0 = 空格，1~4 = 浓度递增。
    private func level(for tokens: Int, maxTokens: Int) -> Int {
        guard tokens > 0 else { return 0 }
        let ratio = Double(tokens) / Double(maxTokens)
        if ratio <= 0.25 { return 1 }
        if ratio <= 0.50 { return 2 }
        if ratio <= 0.75 { return 3 }
        return 4
    }

    // 中性灰阶让用量图保持安静，把彩色留给连接健康状态。
    private func cellColor(level: Int) -> Color {
        switch level {
        case 1: return Color.primary.opacity(0.18)
        case 2: return Color.primary.opacity(0.32)
        case 3: return Color.primary.opacity(0.48)
        case 4: return Color.primary.opacity(0.66)
        default: return Color.primary.opacity(0.10)
        }
    }

    var body: some View {
        GeometryReader { proxy in
            let hasBottomCaption = legendPlacement == .bottom
            let layout = layout(
                for: proxy.size.width,
                height: proxy.size.height,
                includesBottomCaption: hasBottomCaption
            )
            let weeks = grid(weeks: layout.weekCount)
            let maxTokens = maxTokens(in: weeks)

            VStack(alignment: .leading, spacing: Self.contentSpacing) {
                heatmapContent(weeks: weeks, maxTokens: maxTokens, cellSize: layout.cellSize)

                if hasBottomCaption {
                    bottomCaption(weekCount: layout.weekCount, cellSize: layout.cellSize)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
        }
        .frame(minHeight: minimumHeight)
        .onAppear {
            guard !appeared else { return }
            withAnimation(.easeOut(duration: 0.5)) { appeared = true }
        }
    }

    private var minimumHeight: CGFloat {
        let gridHeight = cellSize * 7 + cellSpacing * 6
        let captionHeight: CGFloat = legendPlacement == .bottom ? 18 + Self.contentSpacing : 0
        return gridHeight + Self.monthAxisHeight + Self.contentSpacing + captionHeight
    }

    private func heatmapContent(
        weeks: [Week],
        maxTokens: Int,
        cellSize: CGFloat
    ) -> some View {
        HStack(alignment: .top, spacing: Self.axisGap) {
            weekdayAxis(cellSize: cellSize)

            VStack(alignment: .leading, spacing: Self.contentSpacing) {
                weekGrid(weeks: weeks, maxTokens: maxTokens, cellSize: cellSize)
                monthAxis(weeks: weeks, cellSize: cellSize)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func weekGrid(weeks: [Week], maxTokens: Int, cellSize: CGFloat) -> some View {
        HStack(alignment: .top, spacing: cellSpacing) {
            ForEach(weeks) { week in
                VStack(spacing: cellSpacing) {
                    ForEach(0..<7, id: \.self) { row in
                        cell(
                            week.days[row],
                            maxTokens: maxTokens,
                            staggerIndex: week.index,
                            size: cellSize,
                            bubbleAlignment: bubbleAlignment(
                                weekIndex: week.index,
                                weekCount: weeks.count
                            )
                        )
                    }
                }
            }
        }
    }

    private enum BubbleEdge {
        case leading, center, trailing
    }

    /// 悬浮气泡默认居中；靠边的周改为对齐内侧，避免被卡片边缘裁剪。
    private func bubbleAlignment(weekIndex: Int, weekCount: Int) -> Alignment {
        if weekIndex < 2 { return .topLeading }
        if weekIndex >= weekCount - 2 { return .topTrailing }
        return .top
    }

    private func weekdayAxis(cellSize: CGFloat) -> some View {
        VStack(spacing: cellSpacing) {
            ForEach(0..<7, id: \.self) { index in
                Text(weekdayLabel(for: index))
                    .font(.system(size: 9, weight: .regular))
                    .foregroundStyle(.tertiary)
                    .frame(width: Self.weekdayAxisWidth, height: cellSize, alignment: .leading)
            }
        }
    }

    private func monthAxis(weeks: [Week], cellSize: CGFloat) -> some View {
        HStack(alignment: .top, spacing: cellSpacing) {
            ForEach(weeks) { week in
                Text(monthLabel(for: week))
                    .font(.system(size: 9, weight: .regular))
                    .foregroundStyle(.tertiary)
                    .frame(width: cellSize, alignment: .leading)
                    .fixedSize(horizontal: true, vertical: false)
                    .accessibilityHidden(monthLabel(for: week).isEmpty)
            }
        }
        .frame(height: Self.monthAxisHeight, alignment: .top)
    }

    private func layout(
        for width: CGFloat,
        height: CGFloat,
        includesBottomCaption: Bool
    ) -> Layout {
        let gridWidth = width - Self.weekdayAxisWidth - Self.axisGap
        let captionHeight: CGFloat = includesBottomCaption ? 18 + Self.contentSpacing : 0
        let gridHeight = height - Self.monthAxisHeight - Self.contentSpacing - captionHeight
        let fittedHeight = (gridHeight - 6 * cellSpacing) / 7
        let targetCellSize = min(18, max(4, fittedHeight))
        let fitWeekCount = Int((gridWidth + cellSpacing) / (targetCellSize + cellSpacing))
        let weekCount = min(Self.maximumWeeks, max(Self.minimumWeeks, fitWeekCount))
        let fittedWidth = (gridWidth - CGFloat(weekCount - 1) * cellSpacing) / CGFloat(weekCount)
        return Layout(weekCount: weekCount, cellSize: min(targetCellSize, max(4, fittedWidth)))
    }

    @ViewBuilder
    private func cell(
        _ gridDay: GridDay?,
        maxTokens: Int,
        staggerIndex: Int,
        size: CGFloat,
        bubbleAlignment: Alignment = .top
    ) -> some View {
        if let gridDay {
            let lvl = level(for: gridDay.tokens, maxTokens: maxTokens)
            let isToday = calendar.isDate(gridDay.day, inSameDayAs: todayStart)
            let isHoveredCell = hovered == gridDay
            heatmapSquare(level: lvl, size: size)
                .overlay(
                    RoundedRectangle(cornerRadius: cellCornerRadius(for: size))
                        .strokeBorder(isToday ? Color.primary.opacity(0.72) : .clear, lineWidth: 1.2)
                )
                .overlay(alignment: bubbleAlignment) {
                    if isHoveredCell {
                        hoverBubble(for: gridDay)
                            .offset(y: -(size / 2 + 16))
                            .transition(.opacity.combined(with: .move(edge: .bottom)))
                    }
                }
                // 按周 stagger：越新的周越晚填充（一次性入场动画）。
                .opacity(appeared ? 1 : 0)
                .scaleEffect(appeared ? (isHoveredCell ? 1.32 : 1) : 0.4)
                .offset(y: isHoveredCell ? -2 : 0)
                .shadow(color: isHoveredCell ? Color.black.opacity(0.3) : .clear, radius: 5, y: 2)
                .zIndex(isHoveredCell ? 2 : 0)
                .animation(.spring(duration: 0.5).delay(Double(staggerIndex) * 0.02), value: appeared)
                .animation(.spring(duration: 0.18), value: isHoveredCell)
                .onHover { inside in
                    hovered = inside ? gridDay : (hovered == gridDay ? nil : hovered)
                }
        } else {
            // 空格（未来日期/网格留白）——只占位。
            Color.clear
                .frame(width: size, height: size)
        }
    }

    /// 悬浮在格子上方的即时标签：日期与 Token 分两行，Token 加粗为主信息。
    /// 使用字面不透明深色——任何系统材质/动态色语义都可能引入透明度。
    private func hoverBubble(for gridDay: GridDay) -> some View {
        VStack(spacing: 1) {
            Text(Self.captionDateFormatter.string(from: gridDay.day))
                .font(.system(size: 9))
                .foregroundStyle(.secondary)
            Text("\(formatTokens(gridDay.tokens)) 请求 Token")
                .font(.system(size: 11, weight: .semibold).monospacedDigit())
                .foregroundStyle(.primary)
        }
        .padding(.horizontal, 9)
        .padding(.vertical, 5)
        .background(
            Color(red: 0.12, green: 0.12, blue: 0.13),
            in: RoundedRectangle(cornerRadius: 7, style: .continuous)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .strokeBorder(Color.primary.opacity(0.12), lineWidth: 0.5)
        )
        .shadow(color: .black.opacity(0.16), radius: 5, y: 2)
        .fixedSize()
    }

    private func bottomCaption(weekCount: Int, cellSize: CGFloat) -> some View {
        HStack(spacing: 6) {
            if let hovered {
                Text("\(Self.captionDateFormatter.string(from: hovered.day)) · \(formatTokens(hovered.tokens)) 请求 Token")
            } else {
                Text("最近 \(weekCount) 周")
            }

            Spacer(minLength: 8)
            intensityLegend(cellSize: cellSize)
        }
        .font(.system(size: 10))
        .foregroundStyle(.secondary)
        .frame(maxWidth: .infinity)
    }

    private func intensityLegend(cellSize: CGFloat) -> some View {
        HStack(spacing: 4) {
            Text("少")
            ForEach(0..<5, id: \.self) { level in
                heatmapSquare(level: level, size: min(cellSize, 10))
            }
            Text("多")
        }
        .font(.system(size: 9))
        .fixedSize()
    }

    private func weekdayLabel(for index: Int) -> String {
        switch index {
        case 0: return "周一"
        case 2: return "周三"
        case 4: return "周五"
        default: return ""
        }
    }

    private func monthLabel(for week: Week) -> String {
        let days = week.days.compactMap { $0?.day }
        guard let monthStart = days.first(where: { calendar.component(.day, from: $0) == 1 }) else {
            return ""
        }
        return Self.monthFormatter.string(from: monthStart)
    }

    private func heatmapSquare(level: Int, size: CGFloat) -> some View {
        RoundedRectangle(cornerRadius: cellCornerRadius(for: size))
            .fill(cellColor(level: level))
            .frame(width: size, height: size)
    }

    private func cellCornerRadius(for size: CGFloat) -> CGFloat {
        max(2, min(4, size * 0.2))
    }

    private func formatTokens(_ n: Int) -> String {
        if n >= 1_000_000 {
            return String(format: "%.1fM", Double(n) / 1_000_000)
        } else if n >= 1_000 {
            return String(format: "%.1fK", Double(n) / 1_000)
        }
        return "\(n)"
    }

    private static let captionDateFormatter: DateFormatter = {
        let f = DateFormatter()
        f.calendar = Calendar(identifier: .gregorian)
        f.timeZone = TimeZone(identifier: "UTC")!
        f.locale = Locale.current
        f.setLocalizedDateFormatFromTemplate("MMMd")
        return f
    }()

    private static let monthFormatter: DateFormatter = {
        let f = DateFormatter()
        f.calendar = Calendar(identifier: .gregorian)
        f.timeZone = TimeZone(identifier: "UTC")!
        f.locale = Locale.current
        f.setLocalizedDateFormatFromTemplate("MMM")
        return f
    }()
}
