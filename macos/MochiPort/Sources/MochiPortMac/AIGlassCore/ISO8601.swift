import Foundation

public enum ISO8601 {
    nonisolated(unsafe) private static let withFrac: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
    nonisolated(unsafe) private static let plain: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f
    }()
    public static func date(_ s: String) -> Date? {
        withFrac.date(from: s) ?? plain.date(from: s)
    }
}

public extension Calendar {
    /// UTC calendar，用于不受时区影响的测试和统计。
    static let utc: Calendar = {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "UTC")!
        return c
    }()
}
