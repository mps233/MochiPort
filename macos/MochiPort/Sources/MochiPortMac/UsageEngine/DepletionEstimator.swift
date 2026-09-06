import Foundation

/// 额度耗尽预测结果。
public struct Depletion: Equatable, Sendable {
    /// 预测针对哪个窗口（5h/周/日）。
    public let kind: LimitWindow.Kind
    /// 按当前趋势达到 100% 的预计时刻。
    public let etaTo100: Date
    /// 是否会在重置前耗尽（resetsAt 存在且 etaTo100 < resetsAt）。
    public let willDepleteBeforeReset: Bool
    /// 该窗口的重置时刻（用于基于比例的触发判断）。未知为 nil。
    public let resetsAt: Date?
    public init(kind: LimitWindow.Kind, etaTo100: Date, willDepleteBeforeReset: Bool, resetsAt: Date? = nil) {
        self.kind = kind
        self.etaTo100 = etaTo100
        self.willDepleteBeforeReset = willDepleteBeforeReset
        self.resetsAt = resetsAt
    }
}

/// 从使用率（%）时间序列按线性趋势估计耗尽时点的纯函数集合。
public enum DepletionEstimator {
    /// 用最小二乘线性回归求 %/min 斜率，估计到达 100% 的时刻。
    ///
    /// 估计条件（任一不满足返回 nil）：
    /// - 样本 ≥ 3
    /// - 时间跨度 ≥ 5 分钟
    /// - 斜率 > 0.05 %/min
    ///
    /// `etaTo100 = now + (100 - 最新%) / slope` 分钟。
    /// `willDepleteBeforeReset = resetsAt != nil && etaTo100 < resetsAt`.
    /// `kind` 原样写入结果 Depletion（默认 .session5h——分钟级斜率估计的主要用途）。
    public static func estimate(samples: [(Date, Double)], resetsAt: Date?, now: Date,
                                kind: LimitWindow.Kind = .session5h) -> Depletion? {
        guard samples.count >= 3 else { return nil }
        let sorted = samples.sorted { $0.0 < $1.0 }
        guard let first = sorted.first, let last = sorted.last else { return nil }
        let rangeMinutes = last.0.timeIntervalSince(first.0) / 60
        guard rangeMinutes >= 5 else { return nil }

        // x = 分钟（以首个样本为基准），y = percent
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

    /// 日粒度消耗率（%/day）估计——用于周窗口。
    ///
    /// 相邻快照对的 delta 除以实际天数（dayDiff）得到 %/day。
    /// 这样即使快照间隔是 1 天、2 天、5 天不均匀，也能无失真地归一化。
    /// 跨越重置出现负数的对（delta ≤ 0）和 dayDiff < 0.5 的对会被排除。
    /// 正比率的对不足 1 个时返回 nil。
    /// `snapshots` 是 (day, percent) 对——调用方不必按 day 升序传入，内部会排序。
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

    /// 周额度的日粒度耗尽预测。
    ///
    /// `daysTo100 = (100 - current) / rate` → `etaTo100 = now + daysTo100 天`。
    /// rate 很小（≤ 0.5%/日）时视为噪声返回 nil。current 已达 100% 也返回 nil。
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
