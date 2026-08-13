import Foundation

@MainActor
final class AppModel: ObservableObject {
    @Published var selection: AppSection? = .overview
    @Published private(set) var serviceStatus: ServiceStatus = .checking
    @Published private(set) var lastCheckedAt: Date?

    private let apiClient: APIClient
    private let fixtureStatus: ServiceStatus?

    init(apiClient: APIClient = APIClient(), fixtureStatus: ServiceStatus? = nil) {
        self.apiClient = apiClient
        self.fixtureStatus = fixtureStatus
    }

    func refresh() async {
        // Preview runs use deterministic state and must never contact or
        // change the user's daemon.
        if let fixtureStatus {
            serviceStatus = fixtureStatus
            lastCheckedAt = Date()
            return
        }

        serviceStatus = .checking
        do {
            let probe = try await apiClient.probe()
            switch probe {
            case let .versioned(health):
                serviceStatus = health.ready ? .available : .unavailable("Service is starting")
            case .legacy:
                serviceStatus = .bridgeAvailable
            }
        } catch {
            serviceStatus = .unavailable(error.localizedDescription)
        }
        lastCheckedAt = Date()
    }
}

enum ServiceStatus: Equatable {
    case checking
    case available
    case bridgeAvailable
    case unavailable(String)

    var title: String {
        switch self {
        case .checking: "Checking"
        case .available: "Available"
        case .bridgeAvailable: "Compatible Service"
        case .unavailable: "Unavailable"
        }
    }

    var detail: String {
        switch self {
        case .checking: "Connecting to the local service"
        case .available: "The local service is ready"
        case .bridgeAvailable: "Update the service to enable the versioned management API"
        case let .unavailable(message): message
        }
    }

    var symbol: String {
        switch self {
        case .checking: "arrow.trianglehead.2.clockwise.rotate.90"
        case .available: "checkmark.circle.fill"
        case .bridgeAvailable: "arrow.triangle.2.circlepath.circle.fill"
        case .unavailable: "exclamationmark.triangle.fill"
        }
    }

    var tint: StatusTint {
        switch self {
        case .checking: .secondary
        case .available: .positive
        case .bridgeAvailable: .caution
        case .unavailable: .negative
        }
    }
}

enum StatusTint {
    case secondary
    case positive
    case caution
    case negative
}

enum AppSection: String, CaseIterable, Identifiable {
    case overview
    case codex
    case sessions
    case messaging
    case gateway
    case requestLogs

    var id: String { rawValue }

    var title: String {
        switch self {
        case .overview: "Overview"
        case .codex: "Codex"
        case .sessions: "Sessions"
        case .messaging: "Messaging Channels"
        case .gateway: "AI Gateway"
        case .requestLogs: "Request Logs"
        }
    }

    var symbol: String {
        switch self {
        case .overview: "rectangle.grid.1x2"
        case .codex: "chevron.left.forwardslash.chevron.right"
        case .sessions: "clock.arrow.circlepath"
        case .messaging: "bubble.left.and.bubble.right"
        case .gateway: "point.3.connected.trianglepath.dotted"
        case .requestLogs: "list.bullet.rectangle"
        }
    }

    var group: AppSectionGroup {
        switch self {
        case .overview: .overview
        case .codex, .sessions: .workspace
        case .messaging, .gateway, .requestLogs: .connections
        }
    }
}

enum AppSectionGroup: String, CaseIterable, Identifiable {
    case overview
    case workspace
    case connections

    var id: String { rawValue }

    var title: String? {
        switch self {
        case .overview: nil
        case .workspace: "Workspace"
        case .connections: "Connections"
        }
    }

    var sections: [AppSection] {
        AppSection.allCases.filter { $0.group == self }
    }
}
