import Foundation
import Observation

@MainActor
@Observable
public final class EventLog {
    public struct Record: Identifiable {
        public let id: UUID
        public let event: HUDEvent
        public let date: Date

        public init(id: UUID = UUID(), event: HUDEvent, date: Date) {
            self.id = id
            self.event = event
            self.date = date
        }
    }

    public static let cap = 30

    public private(set) var records: [Record] = []

    public init() {}

    /// 이벤트를 기록 앞에 추가한다. cap(30) 초과 시 오래된 것을 제거한다.
    public func append(_ event: HUDEvent, at date: Date = Date()) {
        records.insert(Record(event: event, date: date), at: 0)
        if records.count > Self.cap {
            records.removeSubrange(Self.cap...)
        }
    }
}
