import Foundation

/// 한도 소진 예측 결과.
public struct Depletion: Equatable, Sendable {
    /// 어느 윈도우(5h/주간/일일)에 대한 예측인지.
    public let kind: LimitWindow.Kind
    /// 현재 추세로 100%에 도달할 추정 시각.
    public let etaTo100: Date
    /// 리셋보다 먼저 소진될지 여부 (resetsAt이 있고 etaTo100 < resetsAt).
    public let willDepleteBeforeReset: Bool
    /// 해당 윈도우의 리셋 시각 (비율 기반 발화 판정에 사용). 모르면 nil.
    public let resetsAt: Date?
    public init(kind: LimitWindow.Kind, etaTo100: Date, willDepleteBeforeReset: Bool, resetsAt: Date? = nil) {
        self.kind = kind
        self.etaTo100 = etaTo100
        self.willDepleteBeforeReset = willDepleteBeforeReset
        self.resetsAt = resetsAt
    }
}

/// 사용률(%) 시계열에서 선형 추세로 소진 시점을 추정하는 순수 함수 묶음.
public enum DepletionEstimator {
    /// 최소제곱 선형 회귀로 %/min 기울기를 구해 100% 도달 시각을 추정한다.
    ///
    /// 추정 조건 (하나라도 불충족 시 nil):
    /// - 샘플 ≥ 3
    /// - 시간 범위 ≥ 5분
    /// - 기울기 > 0.05 %/min
    ///
    /// `etaTo100 = now + (100 - 최신%) / slope` 분.
    /// `willDepleteBeforeReset = resetsAt != nil && etaTo100 < resetsAt`.
    /// `kind`는 결과 Depletion에 그대로 실린다 (기본 .session5h — 분 단위 기울기 추정의 주 용도).
    public static func estimate(samples: [(Date, Double)], resetsAt: Date?, now: Date,
                                kind: LimitWindow.Kind = .session5h) -> Depletion? {
        guard samples.count >= 3 else { return nil }
        let sorted = samples.sorted { $0.0 < $1.0 }
        guard let first = sorted.first, let last = sorted.last else { return nil }
        let rangeMinutes = last.0.timeIntervalSince(first.0) / 60
        guard rangeMinutes >= 5 else { return nil }

        // x = 분(첫 샘플 기준), y = percent
        let xs = sorted.map { $0.0.timeIntervalSince(first.0) / 60 }
        let ys = sorted.map { $0.1 }
        let n = Double(sorted.count)
        let sumX = xs.reduce(0, +)
        let sumY = ys.reduce(0, +)
        let sumXY = zip(xs, ys).reduce(0) { $0 + $1.0 * $1.1 }
        let sumXX = xs.reduce(0) { $0 + $1 * $1 }
        let denom = n * sumXX - sumX * sumX
        guard denom != 0 else { return nil }
        let slope = (n * sumXY - sumX * sumY) / denom  // %/min
        guard slope > 0.05 else { return nil }

        let latest = ys.last!
        let minutesTo100 = (100 - latest) / slope
        guard minutesTo100 > 0 else { return nil }
        let eta = now.addingTimeInterval(minutesTo100 * 60)
        let willDeplete = resetsAt.map { eta < $0 } ?? false
        return Depletion(kind: kind, etaTo100: eta, willDepleteBeforeReset: willDeplete, resetsAt: resetsAt)
    }

    /// 일 단위 소모율(%/day) 추정 — 주간 윈도우용.
    ///
    /// 인접 스냅샷 페어의 delta를 실제 일수(dayDiff)로 나눠 %/day를 구한다.
    /// 이렇게 하면 스냅샷 사이 간격이 1일·2일·5일처럼 불균등해도 왜곡 없이 정규화된다.
    /// 리셋을 걸쳐 음수가 되는 페어(delta ≤ 0)나 dayDiff < 0.5인 페어는 제외.
    /// 양수 비율 페어가 1개 미만이면 nil.
    /// `snapshots`는 (day, percent) 쌍 — 호출자가 일 오름차순으로 넘기지 않아도 내부 정렬.
    public static func weeklyDailyRate(snapshots: [(day: Date, percent: Double)]) -> Double? {
        guard snapshots.count >= 2 else { return nil }
        let sorted = snapshots.sorted { $0.day < $1.day }
        var positives: [Double] = []
        for i in 1..<sorted.count {
            let dayDiff = sorted[i].day.timeIntervalSince(sorted[i - 1].day) / 86400
            guard dayDiff >= 0.5 else { continue }
            let delta = sorted[i].percent - sorted[i - 1].percent
            if delta > 0 { positives.append(delta / dayDiff) }
        }
        guard positives.count >= 1 else { return nil }
        return positives.reduce(0, +) / Double(positives.count)
    }

    /// 주간 한도의 일 단위 소진 예측.
    ///
    /// `daysTo100 = (100 - current) / rate` → `etaTo100 = now + daysTo100일`.
    /// rate가 미미하면(≤ 0.5%/일) 노이즈로 보고 nil. current가 이미 100% 이상이어도 nil.
    /// `willDepleteBeforeReset = resetsAt != nil && etaTo100 < resetsAt`.
    public static func weeklyDepletion(current: Double, rate: Double, resetsAt: Date?, now: Date) -> Depletion? {
        guard rate > 0.5 else { return nil }
        let remaining = 100 - current
        guard remaining > 0 else { return nil }
        let daysTo100 = remaining / rate
        let eta = now.addingTimeInterval(daysTo100 * 24 * 3600)
        let willDeplete = resetsAt.map { eta < $0 } ?? false
        return Depletion(kind: .weekly, etaTo100: eta, willDepleteBeforeReset: willDeplete, resetsAt: resetsAt)
    }
}
