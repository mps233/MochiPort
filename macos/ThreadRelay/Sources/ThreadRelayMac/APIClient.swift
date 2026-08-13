import Foundation

struct HealthResponse: Codable, Equatable {
    let service: String
    let apiMajor: Int
    let ready: Bool
}

private struct LegacyStatusResponse: Codable {
    let service: String
}

enum ServiceProbe: Equatable {
    case versioned(HealthResponse)
    case legacy
}

enum APIClientError: LocalizedError {
    case invalidResponse
    case incompatibleService

    var errorDescription: String? {
        switch self {
        case .invalidResponse: "The local service returned an invalid response."
        case .incompatibleService: "Another service is using the ThreadRelay port."
        }
    }
}

struct APIClient {
    var baseURL = URL(string: "http://127.0.0.1:3847")!
    var session: URLSession = .shared

    init(baseURL: URL = URL(string: "http://127.0.0.1:3847")!, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.session = session
    }

    func probe() async throws -> ServiceProbe {
        let url = baseURL.appending(path: "healthz")
        var request = URLRequest(url: url)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 3

        let (data, response) = try await session.data(for: request)
        guard let httpResponse = response as? HTTPURLResponse else {
            throw APIClientError.invalidResponse
        }

        if httpResponse.statusCode == 404 {
            return try await legacyProbe()
        }
        guard httpResponse.statusCode == 200 else { throw APIClientError.invalidResponse }

        let health: HealthResponse
        do {
            health = try JSONDecoder().decode(HealthResponse.self, from: data)
        } catch {
            throw APIClientError.invalidResponse
        }
        guard health.service == "threadrelay", health.apiMajor == 1 else {
            throw APIClientError.incompatibleService
        }
        return .versioned(health)
    }

    private func legacyProbe() async throws -> ServiceProbe {
        let url = baseURL.appending(path: "api/status")
        var request = URLRequest(url: url)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 3
        let (data, response) = try await session.data(for: request)
        guard let httpResponse = response as? HTTPURLResponse,
              httpResponse.statusCode == 200
        else {
            throw APIClientError.invalidResponse
        }

        let status: LegacyStatusResponse
        do {
            status = try JSONDecoder().decode(LegacyStatusResponse.self, from: data)
        } catch {
            throw APIClientError.invalidResponse
        }
        guard status.service == "threadrelay" || status.service == "codexhub" else {
            throw APIClientError.incompatibleService
        }
        return .legacy
    }
}
