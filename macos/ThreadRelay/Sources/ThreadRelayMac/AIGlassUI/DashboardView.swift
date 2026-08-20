import SwiftUI
import Charts

/// 패널이 열릴 때마다 count를 올려 콘텐츠(`.id`)를 재생성 → onAppear 애니메이션 재생.
@MainActor
@Observable
final class OpenToken {
    var count = 0
}

struct DashboardView: View {
    let store: UsageStore
    /// 30일 영구 통계 (옵셔널 — nil이면 추이 탭의 30일 토글 숨김).
    var statsStore: DailyStatsStore? = nil
    let settings: AppSettings
    /// Real Sub2API cost for the current provider, when the gateway exposes it.
    var providerUsage: ManageProviderUsageResponse? = nil
    /// Most recently used Sub2API channel for the current provider.
    var providerChannel: ManageSub2ApiAccountPoolResponse.Account? = nil
    /// 알림 기록 (옵셔널 — nil이면 기록 탭은 빈 상태).
    var eventLog: EventLog? = nil
    /// 새 버전 배지 (옵셔널 — available일 때만 헤더에 ↓ 표시).
    var updateState: UpdateState? = nil
    /// 톱니바퀴 → 설정 창 열기.
    var onSettings: () -> Void = {}
    /// 탭 전환 등 콘텐츠 높이 변화 시 패널 리사이즈 요청.
    var onResize: () -> Void = {}
    /// 패널이 열릴 때마다 count 증가 — `.id`로 콘텐츠 재생성해 첫 진입 애니메이션 재생.
    var openToken: OpenToken? = nil
    @State private var tab: Tab = .overview
    /// 탭 진입 누적 횟수 — 재진입마다 탭 콘텐츠 identity를 갈아서(`.id`)
    /// 첫 오픈과 동일한 grow+stagger+카운트업 애니메이션을 재생한다.
    /// (전환 애니메이션 도중 같은 탭으로 되돌아오면 떠나던 뷰의 @State가 재사용되어
    ///  게이지가 현재값에서 출렁이는 회귀가 있었다.)
    @State private var tabVisit = 0

    enum Tab: String, CaseIterable, Identifiable {
        case overview = "概览", trends = "趋势", projects = "项目", history = "记录"
        var id: String { rawValue }
    }

    /// 탭이 실제로 바뀔 때 tabVisit을 동기 증가시키는 바인딩.
    /// (.onChange는 뷰 갱신 뒤에 불려 identity 교체가 한 박자 늦어 이중 등장이 생기므로 set에서 처리.)
    private var tabSelection: Binding<Tab> {
        Binding(
            get: { tab },
            set: { newTab in
                guard newTab != tab else { return }
                tabVisit += 1
                tab = newTab
            })
    }

    /// 탭 전환: 통짜 .move 슬라이드 대신 페이드 + 12pt 미세 오프셋 (이동 거리 톤다운).
    private static let tabTransition: AnyTransition =
        .opacity.combined(with: .offset(x: 12))

    var body: some View {
        VStack(spacing: 12) {
            HStack(spacing: 8) {
                Picker("", selection: tabSelection) {
                    ForEach(Tab.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()

                if let release = updateState?.available {
                    Button {
                        NSWorkspace.shared.open(release.url)
                    } label: {
                        Image(systemName: "arrow.down.circle.fill")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(Theme.safeGreen)
                    }
                    .buttonStyle(.plain)
                    .help("下载 v\(release.version) 更新")
                }

                Button(action: onSettings) {
                    Image(systemName: "gearshape")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .help("设置")
            }

            ZStack(alignment: .top) {
                // `.id(tabVisit)`: 진입마다 fresh identity → onAppear/`.task` 등장 애니메이션이
                // 첫 오픈과 동일하게 재생된다 (전환 중 복귀 시 뷰 재사용 방지).
                switch tab {
                case .overview:
                    OverviewTab(
                        store: store,
                        statsStore: statsStore,
                        settings: settings,
                        providerUsage: providerUsage,
                        providerChannel: providerChannel
                    )
                        .id(tabVisit)
                        .transition(Self.tabTransition)
                case .trends:
                    TrendsTab(store: store, statsStore: statsStore, settings: settings, onResize: onResize)
                        .id(tabVisit)
                        .transition(Self.tabTransition)
                case .projects:
                    ProjectsTab(store: store)
                        .id(tabVisit)
                        .transition(Self.tabTransition)
                case .history:
                    HistoryTab(eventLog: eventLog)
                        .id(tabVisit)
                        .transition(Self.tabTransition)
                }
            }
            .animation(.spring(duration: 0.32), value: tab)
        }
        .id(openToken?.count ?? 0)
        .padding(14)
        .frame(width: 320)
        .onChange(of: tab) { onResize() }
    }
}

private struct OverviewTab: View {
    let store: UsageStore
    var statsStore: DailyStatsStore? = nil
    let settings: AppSettings
    var providerUsage: ManageProviderUsageResponse? = nil
    var providerChannel: ManageSub2ApiAccountPoolResponse.Account? = nil

    /// 서비스당 표시 순서: 5h → 주간 → 일일 (호버 카드와 동일 의미론).
    private static let kindOrder: [LimitWindow.Kind] = [.session5h, .weekly, .daily]

    var body: some View {
        let now = Date()
        let enabled: [ServiceID] = [.codex]
        let rows = serviceRows(enabled)
        VStack(spacing: 10) {
            ForEach(rows, id: \.service) { row in
                ServiceRow(service: row.service,
                           windows: row.windows,
                           providerUsage: providerUsage,
                           providerChannel: providerChannel,
                           approxReset: { kind in
                               store.approxFullReset(service: row.service, kind: kind, now: now)
                           },
                           depletions: depletions(for: row.service, now: now),
                           warn: settings.warnThreshold,
                           crit: settings.critThreshold,
                           staggerBase: row.base)
            }
            HStack(spacing: 8) {
                StatCard(value: Double(todayTokens(enabled, now: now)),
                         format: { formatTokens(Int($0)) }, label: "今日 Token")
                StatCard(value: todayCost(enabled, now: now),
                         format: formatCost, label: "今日成本")
                StatCard(value: tokensPerMinute(enabled, now: now),
                         format: { formatTokens(Int($0)) }, label: "当前 Token/分钟")
            }
        }
    }

    /// 서비스의 윈도우별 소진 예측 (kind → Depletion). 5h는 store, 주간은 statsStore 스냅샷 기반.
    /// 둘 다 `willDepleteBeforeReset`일 때만 포함된다 (App.evaluateEvents와 동일 규칙).
    private func depletions(for service: ServiceID, now: Date) -> [LimitWindow.Kind: Depletion] {
        var result: [LimitWindow.Kind: Depletion] = [:]
        if let d = store.depletion(for: service, now: now) { result[d.kind] = d }
        if let statsStore,
           let weekly = store.limits[service]?.first(where: { $0.kind == .weekly }) {
            let snapshots = statsStore.percentSnapshots(service: service, kind: .weekly, days: 8, now: now)
            if let rate = DepletionEstimator.weeklyDailyRate(snapshots: snapshots),
               let w = DepletionEstimator.weeklyDepletion(current: weekly.usedPercent, rate: rate,
                                                          resetsAt: weekly.resetsAt, now: now),
               w.willDepleteBeforeReset {
                result[.weekly] = w
            }
        }
        return result
    }

    /// 서비스별 정렬 윈도우 + 전체 게이지 행 누적 인덱스(stagger 딜레이용).
    private func serviceRows(_ services: [ServiceID])
        -> [(service: ServiceID, windows: [LimitWindow], base: Int)] {
        var result: [(ServiceID, [LimitWindow], Int)] = []
        var base = 0
        for service in services {
            let all = store.limits[service] ?? []
            let sorted = Self.kindOrder.compactMap { kind in all.first { $0.kind == kind } }
            result.append((service, sorted, base))
            base += max(1, sorted.count)
        }
        return result
    }

    private func todayTokens(_ services: [ServiceID], now: Date) -> Int {
        let start = Calendar.current.startOfDay(for: now)
        return store.events
            .filter { services.contains($0.service) && $0.timestamp >= start }
            .reduce(0) { $0 + $1.totalTokens }
    }
    /// 오늘 이벤트의 추정 비용 (API 환산 추정치, enabled 서비스만).
    private func todayCost(_ services: [ServiceID], now: Date) -> Double {
        if let live = liveTodayCost { return live }
        let start = Calendar.current.startOfDay(for: now)
        let today = store.events.filter { services.contains($0.service) && $0.timestamp >= start }
        let multiplier = providerUsage?.usage.effectiveRateMultiplier
            ?? providerUsage?.usage.resolvedRateMultiplier
            ?? 1.0
        return CostEstimator.cost(of: today, multiplier: multiplier)
    }

    private var liveTodayCost: Double? {
        guard let usage = providerUsage?.usage else { return nil }
        if let actual = usage.todayActualCost, actual.isFinite, actual >= 0 {
            return actual
        }
        guard let base = usage.todayCost,
              base.isFinite,
              base >= 0,
              let multiplier = usage.effectiveRateMultiplier ?? usage.resolvedRateMultiplier,
              multiplier.isFinite,
              multiplier >= 0
        else { return nil }
        return base * multiplier
    }
    private func tokensPerMinute(_ services: [ServiceID], now: Date) -> Double {
        services.reduce(0) { $0 + store.tokensPerMinute(service: $1, windowMinutes: 3, now: now) }
    }
    private func formatCost(_ usd: Double) -> String {
        guard usd >= 0.01 else { return "$0" }
        let prefix = liveTodayCost != nil ? "$" : "~$"
        return String(format: "%@%.2f", prefix, usd)
    }
}

private func formatTokens(_ n: Int) -> String {
    switch n {
    case 1_000_000...: return String(format: "%.1fM", Double(n) / 1_000_000)
    case 1_000...: return String(format: "%.0fK", Double(n) / 1_000)
    default: return "\(n)"
    }
}

/// 개요 탭 서비스 블록: 헤더(점+이름+최댓값%) 아래 윈도우별 게이지 행.
private struct ServiceRow: View {
    let service: ServiceID
    let windows: [LimitWindow]
    /// Provider balance used as a useful fallback when Codex has not exposed
    /// its native 5-hour window yet.
    var providerUsage: ManageProviderUsageResponse? = nil
    /// Most recently used Sub2API channel for the current provider.
    var providerChannel: ManageSub2ApiAccountPoolResponse.Account? = nil
    let approxReset: (LimitWindow.Kind) -> Date?
    /// 윈도우 kind별 소진 경고 (해당 윈도우 행 바로 아래에 표시).
    var depletions: [LimitWindow.Kind: Depletion] = [:]
    let warn: Double
    let crit: Double
    /// 전체 개요에서 이 서비스 첫 게이지 행의 인덱스 — delay 0.06*i.
    var staggerBase: Int = 0

    /// Codex logs currently expose the primary 5-hour and secondary weekly
    /// windows. Keep both rows visible while their limit payload is missing so
    /// the dashboard does not collapse and later jump when limits arrive.
    private static let kindOrder: [LimitWindow.Kind] = [.session5h, .weekly, .daily]
    private static let expectedKinds: [LimitWindow.Kind] = [.session5h, .weekly]

    private var displayKinds: [LimitWindow.Kind] {
        // The provider total is shown in the header and the channel balance
        // gets the only gauge row when Codex has not exposed its 5-hour window.
        if shouldShowChannelFallback { return [] }
        var kinds = Self.expectedKinds
        for window in windows where !kinds.contains(window.kind) {
            kinds.append(window.kind)
        }
        return Self.kindOrder.filter { kinds.contains($0) }
    }

    private var primary: LimitWindow? {
        windows.max { $0.usedPercent < $1.usedPercent }
    }

    var body: some View {
        let headerText = shouldShowChannelFallback
            ? (channelRateText ?? "—")
            : (primary.map { Theme.formatUsagePercent($0.usedPercent) } ?? "–")
        let headerColor: Color = shouldShowChannelFallback
            ? (channelRateText == nil ? .secondary : Theme.safeGreen)
            : (primary.map {
                Theme.statusColor(percent: $0.usedPercent, warn: warn, crit: crit)
            } ?? .secondary)
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Circle().fill(Theme.color(for: service)).frame(width: 8, height: 8)
                Text(service.displayName).font(.system(size: 12, weight: .semibold))
                if let channelName = providerChannel?.name,
                   !channelName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                {
                    Text("· \(channelName)")
                        .font(.system(size: 10, weight: .medium))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .layoutPriority(0)
                }
                Spacer()
                Text(headerText)
                    .font(.system(size: 12, weight: .bold).monospacedDigit())
                    .fixedSize()
                    .foregroundStyle(headerColor)
                    .help(shouldShowChannelFallback ? "当前渠道倍率" : "Codex 使用率")
                    .accessibilityLabel(shouldShowChannelFallback
                        ? "当前渠道倍率：\(channelRateText ?? "暂无")"
                        : "使用率：\(primary.map { Theme.formatUsagePercent($0.usedPercent) } ?? "暂无")")
            }
            ForEach(Array(displayKinds.enumerated()), id: \.offset) { idx, kind in
                if let window = windows.first(where: { $0.kind == kind }) {
                    windowRow(window, delay: 0.06 * Double(staggerBase + idx))
                    if let depletion = depletions[window.kind], depletion.willDepleteBeforeReset {
                        depletionLine(depletion)
                    }
                } else {
                    unavailableWindowRow(kind)
                }
            }
            if shouldShowChannelFallback {
                channelBalanceRow
            }
        }
    }

    /// 윈도우별 소진 경고 줄 — 해당 윈도우 행 바로 아래.
    /// 5h: ⚠️ + orange(긴급), 주간: ⚠️ 없이 secondary(차분한 추세 안내).
    @ViewBuilder
    private func depletionLine(_ depletion: Depletion) -> some View {
        let now = Date()
        switch depletion.kind {
        case .weekly:
            Text("照这个趋势，约 \(EventEngine.daysUntil(depletion.etaTo100, from: now)) 天后耗尽")
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
                .padding(.leading, 14)
        default:
            Text("⚠️ 照这个速度，\(EventEngine.countdown(to: depletion.etaTo100, from: now)) 后耗尽 5h 额度")
                .font(.system(size: 10))
                .foregroundStyle(.orange)
                .padding(.leading, 14)
        }
    }

    /// 한 윈도우 줄: 라벨 + 게이지(슈욱) + % + 리셋 카운트다운.
    /// `~`는 **리셋 시각이 근사일 때 시간에만** 붙는다 (사용량 %는 항상 정확값).
    /// 근사 시간은 tertiary로 톤 다운해 %와 시각적으로 분리한다.
    private func windowRow(_ window: LimitWindow, delay: Double) -> some View {
        let tint = Theme.statusColor(percent: window.usedPercent, warn: warn, crit: crit)
        let reset = resetLabel(window)
        return HStack(spacing: 6) {
            Text(window.kind.label)
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(.secondary)
                .frame(width: 26, alignment: .leading)
            GaugeBar(percent: window.usedPercent, tint: tint, appearDelay: delay)
            Text(Theme.formatUsagePercent(window.usedPercent))
                .font(.system(size: 10, weight: .bold).monospacedDigit())
                .foregroundStyle(tint)
                .frame(width: 32, alignment: .trailing)
            Text(reset?.text ?? "")
                .font(.system(size: 9).monospacedDigit())
                .foregroundStyle(reset?.isApprox == true ? AnyShapeStyle(.tertiary)
                                                         : AnyShapeStyle(.secondary))
                .lineLimit(1)
                // "~6d 23h 58m"까지 한 줄 수용 (52에서는 d-포맷이 줄바꿈/잘림).
                .frame(width: 60, alignment: .trailing)
        }
        .padding(.leading, 14)
    }

    /// Placeholder row used when Codex has token data but did not provide
    /// rate-limit metadata in its session log.
    private func unavailableWindowRow(_ kind: LimitWindow.Kind) -> some View {
        return AnyView(
        HStack(spacing: 6) {
            Text(kind.label)
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(.tertiary)
                .frame(width: 26, alignment: .leading)
            DisabledGaugeBar()
            Text("—")
                .font(.system(size: 10, weight: .bold).monospacedDigit())
                .foregroundStyle(.tertiary)
                .frame(width: 32, alignment: .trailing)
            Text("—")
                .font(.system(size: 9).monospacedDigit())
                .foregroundStyle(.tertiary)
                .frame(width: 60, alignment: .trailing)
        }
        .padding(.leading, 14)
        .opacity(0.72)
        .help("暂未获取到 Codex 额度信息")
        .accessibilityLabel("\(kind.label)：暂未获取到 Codex 额度信息")
        )
    }

    private var channelBalanceRow: some View {
        let balanceText = channelBalanceText ?? "—"
        let presentation = providerChannel.map { gatewayQuotaMeterPresentation($0.upstreamBalance) }
        let tint = balanceTint(presentation?.tone)
        return HStack(spacing: 6) {
            Text("渠道余额")
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(.secondary)
                .frame(width: 44, alignment: .leading)
            if let fraction = presentation?.fraction {
                GaugeBar(percent: fraction * 100, tint: tint)
            } else {
                DisabledGaugeBar()
            }
            Text(balanceText)
                .font(.system(size: 9, weight: .semibold).monospacedDigit())
                .foregroundStyle(channelBalanceText == nil
                    ? AnyShapeStyle(.tertiary)
                    : AnyShapeStyle(tint))
                .lineLimit(1)
                .minimumScaleFactor(0.75)
                .frame(width: 60, alignment: .trailing)
        }
        .padding(.leading, 14)
        .help("当前渠道：\(providerChannel?.name ?? "未知")")
        .accessibilityLabel("渠道余额：\(balanceText)，渠道 \(providerChannel?.name ?? "未知")")
    }

    private func balanceTint(_ tone: GatewayQuotaTone?) -> Color {
        switch tone {
        case .normal: return Theme.safeGreen
        case .warning: return .orange
        case .critical: return .red
        case .unavailable, .none: return .secondary
        }
    }

    private var channelBalanceText: String? {
        guard let balance = providerChannel?.upstreamBalance else { return nil }
        return sub2ApiBalanceText(balance)
    }

    private var channelRateText: String? {
        guard let billing = providerChannel?.upstreamBilling else { return nil }
        // Match the quota dock: this is the effective rate returned for the
        // currently selected channel, not the provider-level total rate.
        return providerUsageMultiplierText(
            billing.effectiveRateMultiplier ?? billing.resolvedRateMultiplier
        )
    }

    private var shouldShowChannelFallback: Bool {
        guard !windows.contains(where: { $0.kind == .session5h }),
              let balance = providerChannel?.upstreamBalance
        else { return false }
        return balance.unlimited || (balance.remaining?.isFinite == true)
    }

    /// resetsAt(미래)이 있으면 정확 카운트다운, 없거나 이미 지났으면 근사 리셋(`~` + 톤 다운),
    /// 둘 다 없으면 nil.
    private func resetLabel(_ window: LimitWindow) -> (text: String, isApprox: Bool)? {
        let now = Date()
        if let resets = window.resetsAt, resets > now {
            return (EventEngine.countdown(to: resets, from: now), false)
        }
        if let approx = approxReset(window.kind) {
            return ("~" + EventEngine.countdown(to: approx, from: now), true)
        }
        return nil
    }
}

/// 숫자 카운트업 카드: onAppear 시 0→값을 0.45초 6스텝 보간 (.numericText 전환).
private struct StatCard: View {
    let value: Double
    let format: (Double) -> String
    let label: String
    @State private var displayed: Double = 0

    var body: some View {
        VStack(spacing: 2) {
            Text(format(displayed))
                .font(.system(size: 14, weight: .bold).monospacedDigit())
                .contentTransition(.numericText())
            Text(label).font(.system(size: 9)).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 8)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 10))
        .task {
            displayed = 0
            for step in 1...6 {
                try? await Task.sleep(for: .milliseconds(75))
                guard !Task.isCancelled else { return }
                withAnimation(.easeOut(duration: 0.09)) {
                    displayed = value * Double(step) / 6.0
                }
            }
        }
        .onChange(of: value) { _, newValue in
            withAnimation(.spring(duration: 0.4)) { displayed = newValue }
        }
    }
}

private struct TrendsTab: View {
    let store: UsageStore
    /// nil이면 30일/잔디 토글 숨김.
    var statsStore: DailyStatsStore? = nil
    let settings: AppSettings
    /// 세그먼트 변경 시 잔디(높이 ≠ 차트)로 패널 크기가 달라지므로 리사이즈를 트리거한다.
    var onResize: () -> Void = {}
    @State private var range: Range = .week
    /// 0.0→1.0으로 증가하며 BarMark y값에만 곱해진다.
    /// 축/눈금/레이아웃은 최종 데이터로 고정되어 움직이지 않는다.
    @State private var growFactor: Double = 0

    enum Range: String, CaseIterable, Identifiable {
        case week = "7天", month = "30天", heatmap = "热力图"
        var id: String { rawValue }
        var days: Int {
            switch self {
            case .week: return 7
            case .month: return 30
            case .heatmap: return 105
            }
        }
    }

    /// id가 (day, service)로 안정적이어야 growFactor 변화 시 Chart가 같은 바로 인식해
    /// y값을 보간(차오름)한다. UUID()는 body 평가마다 바뀌어 애니메이션이 끊겼다.
    private struct Point: Identifiable {
        let day: Date
        let service: ServiceID
        let tokens: Int
        var id: String { "\(day.timeIntervalSinceReferenceDate)-\(service.rawValue)" }
    }

    private let enabled: [ServiceID] = [.codex]

    /// 데이터 소스만 분기: 7일은 store(in-memory), 30일은 statsStore(SQLite, UTC day).
    private var rawData: [(day: Date, service: ServiceID, tokens: Int)] {
        let rows: [(day: Date, service: ServiceID, tokens: Int)]
        switch range {
        case .week:
            rows = store.dailyTotalsByService(days: 7, now: Date())
        case .month, .heatmap:
            // 저장 포맷이 UTC day이므로 Calendar.utc로 조회해야 일관성 유지.
            // 잔디 모드에선 차트를 그리지 않으므로 30일치만 조회(미사용).
            rows = statsStore?.dailyTotalsByService(days: 30, now: Date(), calendar: .utc) ?? []
        }
        return rows
    }

    /// 전체 날짜 범위 (x축 도메인 고정: 오늘 기준 days일 전 ~ 오늘).
    private var xDomain: ClosedRange<Date> {
        let now = Date()
        let start = Calendar.current.startOfDay(for: now)
            .addingTimeInterval(-Double(range.days - 1) * 86400)
        let end = Calendar.current.startOfDay(for: now)
            .addingTimeInterval(86400)
        return start...end
    }

    var body: some View {
        VStack(spacing: 8) {
            if statsStore != nil {
                Picker("", selection: $range) {
                    ForEach(Range.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .controlSize(.small)
                .onChange(of: range) { _, _ in
                    startGrow()
                    // 잔디↔차트 높이가 달라 패널 크기 재조정 필요.
                    onResize()
                }
            }
            if range == .heatmap, let statsStore {
                HeatmapView(statsStore: statsStore, enabledServices: [.codex])
            } else {
                chart
            }
        }
        .onAppear { startGrow() }
    }

    /// 0 프레임을 무애니메이션으로 먼저 커밋한 뒤 다음 틱에 1로 스프링 — 같은 트랜잭션에서
    /// 0→1을 연달아 쓰면 리셋이 합쳐져 성장이 생략될 수 있다 (range 전환 시).
    private func startGrow() {
        var tx = Transaction()
        tx.disablesAnimations = true
        withTransaction(tx) { growFactor = 0 }
        Task { @MainActor in
            withAnimation(.spring(duration: 0.8)) { growFactor = 1 }
        }
    }

    private var chart: some View {
        // rawData(이벤트 풀스캔/SQLite 조회)는 body 평가당 1회만 — y축 최댓값도 여기서 도출.
        let rows = rawData
        let data = rows.map { Point(day: $0.day, service: $0.service, tokens: $0.tokens) }
        let services = enabled
        let byDay = Dictionary(grouping: rows, by: { $0.day })
        let maxStack = byDay.values.map { $0.reduce(0) { $0 + $1.tokens } }.max() ?? 0
        let maxY = Double(max(1, maxStack)) * 1.05
        let factor = growFactor
        return Chart(data) { item in
            BarMark(x: .value("日期", item.day, unit: .day),
                    y: .value("Token", Double(item.tokens) * factor))
                .foregroundStyle(by: .value("服务", item.service.displayName))
                .cornerRadius(3)
        }
        .chartForegroundStyleScale(domain: services.map(\.displayName),
                                   range: services.map { Theme.color(for: $0) })
        .chartXScale(domain: xDomain)
        .chartYScale(domain: 0...maxY)
        .chartXAxis {
            // 30일은 일 단위 라벨이 겹치므로 주(7일) 간격으로만 표기.
            if range == .month {
                AxisMarks(values: .stride(by: .day, count: 7)) {
                    AxisValueLabel(format: .dateTime.month(.defaultDigits).day(), centered: false)
                    AxisGridLine()
                }
            } else {
                AxisMarks(values: .stride(by: .day)) {
                    AxisValueLabel(format: .dateTime.day(), centered: true)
                }
            }
        }
        .chartYAxis {
            AxisMarks(position: .leading) { value in
                AxisGridLine()
                AxisTick()
                AxisValueLabel {
                    if let tokens = value.as(Double.self) {
                        Text(formatAxisTokens(tokens))
                    }
                }
            }
        }
        .chartLegend(position: .bottom)
        .font(.system(size: 9))
        .frame(height: 160)
    }

    /// Keep large token totals readable on the axis instead of letting
    /// Swift Charts fall back to scientific notation such as `1.0E8`.
    private func formatAxisTokens(_ value: Double) -> String {
        switch value {
        case 1_000_000_000...:
            return String(format: "%.1fB", value / 1_000_000_000)
        case 1_000_000...:
            return String(format: "%.0fM", value / 1_000_000)
        case 1_000...:
            return String(format: "%.0fK", value / 1_000)
        default:
            return String(Int(value.rounded()))
        }
    }
}

private struct ProjectsTab: View {
    let store: UsageStore

    var body: some View {
        let projects = filteredProjects()
        let grandTotal = max(1, projects.reduce(0) { $0 + $1.total })
        VStack(spacing: 8) {
            if projects.isEmpty {
                Text("最近 7 天暂无数据").font(.caption).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 80)
            }
            ForEach(Array(projects.prefix(6).enumerated()), id: \.element.project) { idx, item in
                VStack(alignment: .leading, spacing: 3) {
                    HStack {
                        Text(item.project).font(.system(size: 11, weight: .medium)).lineLimit(1)
                        Spacer()
                        Text(formatTokens(item.total))
                            .font(.system(size: 11).monospacedDigit())
                            .foregroundStyle(.secondary)
                        Text("\(Int(Double(item.total) / Double(grandTotal) * 100))%")
                            .font(.system(size: 11).monospacedDigit())
                            .foregroundStyle(.secondary)
                    }
                    StackBar(byService: item.byService, total: item.total,
                             appearDelay: 0.06 * Double(idx))
                }
            }
        }
    }

    /// enabled 서비스만 남기고 합계 재계산, 0이 된 프로젝트는 제거.
    private func filteredProjects() -> [(project: String, byService: [ServiceID: Int], total: Int)] {
        store.projectServiceBreakdown(days: 7, now: Date())
            .compactMap { item in
                guard item.total > 0 else { return nil }
                return item
            }
            .sorted { $0.total > $1.total }
    }
}

/// 프로젝트 행의 서비스별 색 비례 세그먼트 스택 (높이 6, Capsule 클립).
/// onAppear 시 폭 0→값으로 자라며 행별 stagger.
private struct StackBar: View {
    let byService: [ServiceID: Int]
    let total: Int
    var appearDelay: Double = 0
    @State private var appeared = false

    var body: some View {
        GeometryReader { geo in
            let denom = Double(max(1, total))
            let factor = appeared ? 1.0 : 0.0
            HStack(spacing: 0) {
                ForEach(ServiceID.allCases) { service in
                    let tokens = byService[service] ?? 0
                    if tokens > 0 {
                        Theme.color(for: service)
                            .frame(width: geo.size.width * Double(tokens) / denom * factor)
                    }
                }
            }
        }
        .frame(height: 6)
        .clipShape(Capsule())
        .onAppear {
            withAnimation(.spring(duration: 0.7).delay(appearDelay)) { appeared = true }
        }
    }
}

/// 记录 tab：recent notifications, newest first.
private struct HistoryTab: View {
    let eventLog: EventLog?

    var body: some View {
        let records = eventLog?.records ?? []
        if records.isEmpty {
            VStack(spacing: 6) {
                Image(systemName: "bell.slash")
                    .font(.system(size: 20))
                    .foregroundStyle(.tertiary)
                Text("暂无通知")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, minHeight: 120)
        } else {
            ScrollView {
                VStack(spacing: 2) {
                    ForEach(records) { record in
                        HistoryRow(record: record)
                    }
                }
            }
            .frame(height: min(CGFloat(records.count) * 42 + 8, 300))
        }
    }
}

private struct HistoryRow: View {
    let record: EventLog.Record
    @State private var hovering = false

    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .short
        f.locale = Locale(identifier: "zh_CN")
        return f
    }()

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: record.event.kind.iconName)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(record.event.kind.iconColor)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 1) {
                Text(record.event.title)
                    .font(.system(size: 11, weight: .semibold))
                Text(record.event.subtitle)
                    .font(.system(size: 9))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 4)
            Text(Self.relativeFormatter.localizedString(for: record.date, relativeTo: Date()))
                .font(.system(size: 9))
                .foregroundStyle(.tertiary)
        }
        .padding(.vertical, 5)
        .padding(.horizontal, 6)
        .background(hovering ? AnyShapeStyle(.quaternary.opacity(0.6)) : AnyShapeStyle(.clear),
                    in: RoundedRectangle(cornerRadius: 8))
        .onHover { hovering = $0 }
    }
}
