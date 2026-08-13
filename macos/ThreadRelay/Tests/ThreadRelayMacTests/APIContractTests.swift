import Foundation
import XCTest

#if canImport(ThreadRelayMac)
@testable import ThreadRelayMac
#elseif canImport(ThreadRelay)
@testable import ThreadRelay
#endif

final class APIContractTests: XCTestCase {
    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    func testHealthFixtureDecodes() throws {
        let data = Data(#"{"service":"threadrelay","apiMajor":1,"ready":true}"#.utf8)
        let health = try JSONDecoder().decode(HealthResponse.self, from: data)

        XCTAssertEqual(
            health,
            HealthResponse(service: "threadrelay", apiMajor: 1, ready: true)
        )
    }

    func testNavigationContainsPhaseZeroSections() {
        XCTAssertEqual(
            AppSection.allCases.map(\.title),
            ["Overview", "Codex", "Sessions", "Messaging Channels", "AI Gateway", "Request Logs"]
        )
    }

    func testServiceProbeKeepsVersionedHealthPayload() {
        let health = HealthResponse(service: "threadrelay", apiMajor: 1, ready: true)
        XCTAssertEqual(ServiceProbe.versioned(health), .versioned(health))
    }

    func testProbeUsesVersionedHealthEndpoint() async throws {
        let recorder = RequestRecorder()
        let client = makeClient { request in
            recorder.record(request.url?.path)
            return MockResponse(
                statusCode: 200,
                json: #"{"service":"threadrelay","apiMajor":1,"ready":true}"#
            )
        }

        let result = try await client.probe()

        XCTAssertEqual(
            result,
            .versioned(HealthResponse(service: "threadrelay", apiMajor: 1, ready: true))
        )
        XCTAssertEqual(recorder.paths, ["/healthz"])
    }

    func testProbeFallsBackToLegacyStatusAfterHealthNotFound() async throws {
        let recorder = RequestRecorder()
        let client = makeClient { request in
            let path = request.url?.path
            recorder.record(path)

            switch path {
            case "/healthz":
                return MockResponse(statusCode: 404, json: #"{"error":"not found"}"#)
            case "/api/status":
                return MockResponse(statusCode: 200, json: #"{"service":"threadrelay"}"#)
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let result = try await client.probe()

        XCTAssertEqual(result, .legacy)
        XCTAssertEqual(recorder.paths, ["/healthz", "/api/status"])
    }

    func testProbeRejectsIncompatibleService() async {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 200,
                json: #"{"service":"another-service","apiMajor":1,"ready":true}"#
            )
        }

        await assertProbeError(.incompatibleService, from: client)
    }

    func testProbeMapsMalformedJSONToInvalidResponse() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 200, json: #"{"service":"threadrelay""#)
        }

        await assertProbeError(.invalidResponse, from: client)
    }

    func testProbeMapsNonSuccessStatusToInvalidResponse() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 503, json: #"{"error":"unavailable"}"#)
        }

        await assertProbeError(.invalidResponse, from: client)
    }

    private func makeClient(
        handler: @escaping MockURLProtocol.Handler
    ) -> APIClient {
        MockURLProtocol.install(handler)

        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [MockURLProtocol.self]
        let session = URLSession(configuration: configuration)
        return APIClient(baseURL: URL(string: "https://threadrelay.test")!, session: session)
    }

    private func assertProbeError(
        _ expectedError: APIClientError,
        from client: APIClient,
        file: StaticString = #filePath,
        line: UInt = #line
    ) async {
        do {
            _ = try await client.probe()
            XCTFail("Expected probe to fail", file: file, line: line)
        } catch let error as APIClientError {
            switch (expectedError, error) {
            case (.invalidResponse, .invalidResponse),
                 (.incompatibleService, .incompatibleService):
                break
            default:
                XCTFail("Expected \(expectedError), received \(error)", file: file, line: line)
            }
        } catch {
            XCTFail("Expected APIClientError, received \(error)", file: file, line: line)
        }
    }
}

private struct MockResponse: Sendable {
    let statusCode: Int
    let body: Data

    init(statusCode: Int, json: String) {
        self.statusCode = statusCode
        body = Data(json.utf8)
    }
}

private final class RequestRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var recordedPaths: [String] = []

    var paths: [String] {
        lock.withLock { recordedPaths }
    }

    func record(_ path: String?) {
        lock.withLock {
            recordedPaths.append(path ?? "")
        }
    }
}

private final class MockURLProtocol: URLProtocol, @unchecked Sendable {
    typealias Handler = @Sendable (URLRequest) -> MockResponse

    private static let lock = NSLock()
    nonisolated(unsafe) private static var installedHandler: Handler?

    static func install(_ handler: @escaping Handler) {
        lock.withLock {
            installedHandler = handler
        }
    }

    static func reset() {
        lock.withLock {
            installedHandler = nil
        }
    }

    override class func canInit(with _: URLRequest) -> Bool {
        true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        guard let handler = Self.lock.withLock({ Self.installedHandler }),
              let url = request.url
        else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }

        let mock = handler(request)
        guard let response = HTTPURLResponse(
                  url: url,
                  statusCode: mock.statusCode,
                  httpVersion: "HTTP/1.1",
                  headerFields: ["Content-Type": "application/json"]
              )
        else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }

        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: mock.body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}
