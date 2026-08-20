import SwiftUI

/// GitHub 잔디 스타일 사용량 히트맵.
///
/// 최근 15주 × 7일(월~일) 그리드. 열 = 주(왼쪽=과거, 오른쪽=최신),
/// 행 = 요일(위=월, 아래=일). 셀 농도 = 일 토큰 / 기간 최대 토큰을 5단계로 양자화.
/// hover 시 그리드 하단에 고정 캡션(날짜 + 토큰)을 표시한다(팝오버 금지).
/// 오늘 셀은 테두리로 강조하고, 등장 시 주 단위 stagger로 농도가 0→값으로 차오른다(1회성).
struct HeatmapView: View {
    let statsStore: DailyStatsStore
    let enabledServices: Set<ServiceID>

    private static let weeks = 15
    private static let cellSize: CGFloat = 9
    private static let cellSpacing: CGFloat = 3
    private static let cellCorner: CGFloat = 2

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
        let index: Int          // 0 = 가장 과거 주, weeks-1 = 최신 주 (stagger 순서)
        let days: [GridDay?]    // 7칸 (월~일)
        var id: Int { index }
    }

    private var calendar: Calendar {
        // day 저장 포맷이 UTC이므로(추이 30일과 동일) UTC 기준으로 격자를 구성한다.
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "UTC")!
        cal.firstWeekday = 2  // 월요일 시작
        return cal
    }

    // 일별 토큰 딕셔너리(UTC 자정 Date 키)와 기간 최대값.
    private var dailyTokens: [Date: Int] {
        let rows = statsStore.dailyTotals(days: Self.weeks * 7, now: Date(),
                                          calendar: calendar, services: enabledServices)
        var dict: [Date: Int] = [:]
        for r in rows { dict[calendar.startOfDay(for: r.day)] = r.tokens }
        return dict
    }

    private var todayStart: Date { calendar.startOfDay(for: Date()) }

    // 15주 격자 구성: 오른쪽(최신) 열이 이번 주가 되도록, 이번 주가 속한 주의 월요일에서
    // (weeks-1)주 전 월요일까지를 시작점으로 잡는다.
    private var grid: [Week] {
        let tokens = dailyTokens
        let today = todayStart
        // 이번 주 월요일.
        let weekday = calendar.component(.weekday, from: today)  // 1=일 … 2=월
        let daysSinceMonday = (weekday + 5) % 7                  // 월=0, 일=6
        guard let thisMonday = calendar.date(byAdding: .day, value: -daysSinceMonday, to: today),
              let firstMonday = calendar.date(byAdding: .day, value: -(Self.weeks - 1) * 7, to: thisMonday)
        else { return [] }

        var weeks: [Week] = []
        for w in 0..<Self.weeks {
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

    private var maxTokens: Int {
        max(1, dailyTokens.values.max() ?? 0)
    }

    // 5단계 양자화: 0 = 빈 셀, 1~4 = 민트 농도.
    private func level(for tokens: Int) -> Int {
        guard tokens > 0 else { return 0 }
        let ratio = Double(tokens) / Double(maxTokens)
        if ratio <= 0.25 { return 1 }
        if ratio <= 0.50 { return 2 }
        if ratio <= 0.75 { return 3 }
        return 4
    }

    // 민트(safeGreen) 계열 농도. 라이트/다크 시맨틱 대비 위해 0단계는 .quaternary.
    private func cellColor(level: Int) -> Color {
        switch level {
        case 1: return Theme.safeGreen.opacity(0.30)
        case 2: return Theme.safeGreen.opacity(0.55)
        case 3: return Theme.safeGreen.opacity(0.78)
        case 4: return Theme.safeGreen.opacity(1.0)
        default: return Color.primary.opacity(0.10)
        }
    }

    var body: some View {
        VStack(spacing: 8) {
            HStack(alignment: .top, spacing: Self.cellSpacing) {
                ForEach(grid) { week in
                    VStack(spacing: Self.cellSpacing) {
                        ForEach(0..<7, id: \.self) { row in
                            cell(week.days[row], staggerIndex: week.index)
                        }
                    }
                }
            }
            caption
        }
        .padding(.vertical, 4)
        .onAppear {
            guard !appeared else { return }
            withAnimation(.easeOut(duration: 0.5)) { appeared = true }
        }
    }

    @ViewBuilder
    private func cell(_ gridDay: GridDay?, staggerIndex: Int) -> some View {
        if let gridDay {
            let lvl = level(for: gridDay.tokens)
            let isToday = calendar.isDate(gridDay.day, inSameDayAs: todayStart)
            RoundedRectangle(cornerRadius: Self.cellCorner)
                .fill(cellColor(level: lvl))
                .frame(width: Self.cellSize, height: Self.cellSize)
                .overlay(
                    RoundedRectangle(cornerRadius: Self.cellCorner)
                        .strokeBorder(isToday ? Theme.safeGreen : .clear, lineWidth: 1.2)
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
                .frame(width: Self.cellSize, height: Self.cellSize)
        }
    }

    // 그리드 하단 고정 캡션. hover 중인 셀의 날짜+토큰, 없으면 범례.
    @ViewBuilder
    private var caption: some View {
        if let hovered {
            HStack(spacing: 6) {
                Text(Self.captionDateFormatter.string(from: hovered.day))
                Text("·")
                Text("\(formatTokens(hovered.tokens)) Token")
            }
            .font(.system(size: 10))
            .foregroundStyle(.secondary)
        } else {
            HStack(spacing: 4) {
                Text("少").font(.system(size: 9)).foregroundStyle(.secondary)
                ForEach(0..<5, id: \.self) { lvl in
                    RoundedRectangle(cornerRadius: Self.cellCorner)
                        .fill(cellColor(level: lvl))
                        .frame(width: Self.cellSize, height: Self.cellSize)
                }
                Text("多").font(.system(size: 9)).foregroundStyle(.secondary)
            }
        }
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
}
