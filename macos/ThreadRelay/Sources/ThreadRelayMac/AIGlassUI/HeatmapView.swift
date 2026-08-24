import SwiftUI

/// GitHub 잔디 스타일 사용량 히트맵.
///
/// 최근 15주 × 7일(월~일) 그리드. 열 = 주(왼쪽=과거, 오른쪽=최신),
/// 행 = 요일(위=월, 아래=일). 셀 농도 = 일 토큰 / 기간 최대 토큰을 5단계로 양자화.
/// hover 시 고정 캡션(날짜 + 토큰)을 표시한다(팝오버 금지).
/// 오늘 셀은 테두리로 강조하고, 등장 시 주 단위 stagger로 농도가 0→값으로 차오른다(1회성).
struct HeatmapView: View {
    enum LegendPlacement {
        case bottom
        case trailing
        case none
    }

    let statsStore: DailyStatsStore
    let enabledServices: Set<ServiceID>
    let cellSize: CGFloat
    let cellSpacing: CGFloat
    let legendPlacement: LegendPlacement

    private static let minimumWeeks = 15
    private static let maximumWeeks = 53
    private static let trailingLegendSpacing: CGFloat = 14
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

    // 그리드의 한 칸 = 하나의 날짜(또는 빈 칸).
    private struct GridDay: Identifiable, Equatable {
        let day: Date
        let tokens: Int
        var id: TimeInterval { day.timeIntervalSinceReferenceDate }
    }

    // 그리드 컬럼(주) — 각 주는 월~일 7칸. 미래 날짜는 nil로 비운다.
    private struct Week: Identifiable {
        let index: Int          // 0 = 가장 과거 주, 마지막 = 최신 주 (stagger 순서)
        let days: [GridDay?]    // 7칸 (월~일)
        var id: Int { index }
    }

    private struct Layout {
        let weekCount: Int
        let cellSize: CGFloat
    }

    private var calendar: Calendar {
        // day 저장 포맷이 UTC이므로(추이 30일과 동일) UTC 기준으로 격자를 구성한다.
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "UTC")!
        cal.firstWeekday = 2  // 월요일 시작
        return cal
    }

    // 일별 토큰 딕셔너리(UTC 자정 Date 키).
    private func dailyTokens(weeks: Int) -> [Date: Int] {
        let rows = statsStore.dailyTotals(days: weeks * 7, now: Date(),
                                          calendar: calendar, services: enabledServices)
        var dict: [Date: Int] = [:]
        for r in rows { dict[calendar.startOfDay(for: r.day)] = r.tokens }
        return dict
    }

    private var todayStart: Date { calendar.startOfDay(for: Date()) }

    // 오른쪽(최신) 열이 이번 주가 되도록, 이번 주가 속한 월요일에서
    // (weekCount-1)주 전 월요일까지를 시작점으로 잡는다.
    private func grid(weeks weekCount: Int) -> [Week] {
        let tokens = dailyTokens(weeks: weekCount)
        let today = todayStart
        // 이번 주 월요일.
        let weekday = calendar.component(.weekday, from: today)  // 1=일 … 2=월
        let daysSinceMonday = (weekday + 5) % 7                  // 월=0, 일=6
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
                    days.append(nil)  // 미래 날짜는 빈 칸
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

    // 5단계 양자화: 0 = 빈 셀, 1~4 = 민트 농도.
    private func level(for tokens: Int, maxTokens: Int) -> Int {
        guard tokens > 0 else { return 0 }
        let ratio = Double(tokens) / Double(maxTokens)
        if ratio <= 0.25 { return 1 }
        if ratio <= 0.50 { return 2 }
        if ratio <= 0.75 { return 3 }
        return 4
    }

    // Neutral grayscale levels keep usage visualizations quiet and let the
    // state indicators reserve color for connection health.
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
            let reservedTrailingWidth = legendPlacement == .trailing
                ? cellSize + Self.trailingLegendSpacing
                : 0
            let layout = layout(
                for: proxy.size.width - reservedTrailingWidth,
                height: proxy.size.height,
                includesBottomCaption: hasBottomCaption
            )
            let weeks = grid(weeks: layout.weekCount)
            let maxTokens = maxTokens(in: weeks)

            if legendPlacement == .trailing {
                HStack(alignment: .center, spacing: Self.trailingLegendSpacing) {
                    heatmapContent(weeks: weeks, maxTokens: maxTokens, cellSize: layout.cellSize)
                    trailingCaption(cellSize: layout.cellSize)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
            } else {
                VStack(alignment: .leading, spacing: Self.contentSpacing) {
                    heatmapContent(weeks: weeks, maxTokens: maxTokens, cellSize: layout.cellSize)

                    if hasBottomCaption {
                        bottomCaption(weekCount: layout.weekCount, cellSize: layout.cellSize)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
            }
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
                            size: cellSize
                        )
                    }
                }
            }
        }
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
        size: CGFloat
    ) -> some View {
        if let gridDay {
            let lvl = level(for: gridDay.tokens, maxTokens: maxTokens)
            let isToday = calendar.isDate(gridDay.day, inSameDayAs: todayStart)
            heatmapSquare(level: lvl, size: size)
                .overlay(
                    RoundedRectangle(cornerRadius: cellCornerRadius(for: size))
                        .strokeBorder(isToday ? Color.primary.opacity(0.72) : .clear, lineWidth: 1.2)
                )
                // 주 단위 stagger: 최신 주일수록 살짝 늦게 차오른다(1회성).
                .opacity(appeared ? 1 : 0)
                .scaleEffect(appeared ? 1 : 0.4)
                .animation(.spring(duration: 0.5).delay(Double(staggerIndex) * 0.02), value: appeared)
                .onHover { inside in
                    hovered = inside ? gridDay : (hovered == gridDay ? nil : hovered)
                }
        } else {
            // 빈 칸(미래/격자 패딩) — 자리만 차지.
            Color.clear
                .frame(width: size, height: size)
        }
    }

    private func bottomCaption(weekCount: Int, cellSize: CGFloat) -> some View {
        HStack(spacing: 6) {
            if let hovered {
                Text("\(Self.captionDateFormatter.string(from: hovered.day)) · \(formatTokens(hovered.tokens)) Token")
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

    private func trailingCaption(cellSize: CGFloat) -> some View {
        VStack(spacing: 4) {
            Text("多").font(.system(size: 9)).foregroundStyle(.secondary)
            ForEach((0..<5).reversed(), id: \.self) { level in
                heatmapSquare(level: level, size: cellSize)
            }
            Text("少").font(.system(size: 9)).foregroundStyle(.secondary)
        }
        .fixedSize()
        .accessibilityLabel(hovered.map { "\(Self.captionDateFormatter.string(from: $0.day)) \(formatTokens($0.tokens)) Token" } ?? "使用强度")
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
