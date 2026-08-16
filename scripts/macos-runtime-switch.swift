import Foundation

private struct HarnessError: Error, LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

private struct ControlFile: Decodable {
    let managementToken: String
}

private struct RestartPayload: Decodable {
    let ok: Bool
    let state: String
}

private struct LifecyclePayload: Decodable {
    struct Service: Decodable {
        let instanceId: String
        let pid: Int
        let startedAtMs: Int64
    }

    struct Runtime: Decodable {
        let state: String
        let buildNumber: Int
    }

    struct Management: Decodable {
        let leaseGeneration: Int64?
    }

    let service: Service
    let executable: String
    let bind: String
    let runtime: Runtime
    let management: Management
}

private struct HTTPClient {
    let baseURL = URL(string: "http://127.0.0.1:3847")!
    let token: String

    init(controlFile: URL) throws {
        let data = try Data(contentsOf: controlFile)
        token = try JSONDecoder().decode(ControlFile.self, from: data).managementToken
    }

    func send<Response: Decodable>(
        _ path: String,
        method: String = "GET",
        body: Data? = nil
    ) async throws -> Response {
        let (status, data) = try await sendRaw(path, method: method, body: body)
        guard (200...299).contains(status) else {
            throw HarnessError(message: "HTTP \(status) for \(path): \(String(decoding: data, as: UTF8.self))")
        }
        return try JSONDecoder().decode(Response.self, from: data)
    }

    func sendRaw(_ path: String, method: String = "GET", body: Data? = nil) async throws -> (Int, Data) {
        var request = URLRequest(url: baseURL.appending(path: path))
        request.httpMethod = method
        request.timeoutInterval = 15
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        if let body {
            request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let response = response as? HTTPURLResponse else {
            throw HarnessError(message: "invalid HTTP response for \(path)")
        }
        return (response.statusCode, data)
    }

    func restart(
        instanceId: String,
        installationId: String,
        leaseGeneration: Int64,
        force: Bool
    ) async throws {
        let body = try JSONSerialization.data(withJSONObject: [
            "installationId": installationId,
            "daemonInstanceId": instanceId,
            "force": force,
            "leaseGeneration": leaseGeneration,
        ])
        let response: RestartPayload = try await send(
            "api/v1/manage/lifecycle/restart",
            method: "POST",
            body: body
        )
        guard response.ok else {
            throw HarnessError(message: "lifecycle restart was not accepted (state=\(response.state))")
        }
    }

    func lifecycle() async throws -> LifecyclePayload {
        try await send("api/v1/manage/lifecycle")
    }

    func commit(instanceId: String, installationId: String, leaseGeneration: Int64) async throws {
        let body = try JSONSerialization.data(withJSONObject: [
            "installationId": installationId,
            "daemonInstanceId": instanceId,
            "leaseGeneration": leaseGeneration,
        ])
        let (status, data) = try await sendRaw(
            "api/v1/manage/lifecycle/runtime-switch/commit",
            method: "POST",
            body: body
        )
        guard (200...299).contains(status) else {
            throw HarnessError(message: "runtime commit failed HTTP \(status): \(String(decoding: data, as: UTF8.self))")
        }
    }
}

private func run(_ executable: URL, _ arguments: [String]) throws -> CommandResult {
    let process = Process()
    let output = Pipe()
    process.executableURL = executable
    process.arguments = arguments
    process.standardOutput = output
    process.standardError = output
    try process.run()
    process.waitUntilExit()
    let data = output.fileHandleForReading.readDataToEndOfFile()
    return CommandResult(
        exitCode: process.terminationStatus,
        output: String(decoding: data, as: UTF8.self)
    )
}

private func requiredArgument(_ arguments: [String], _ index: Int) throws -> String {
    guard arguments.indices.contains(index) else {
        throw HarnessError(message: "missing argument \(index)")
    }
    return arguments[index]
}

@main
private struct ForceRuntimeSwitchHarness {
    static func main() async {
        do {
            let arguments = Array(CommandLine.arguments.dropFirst())
            let action = try requiredArgument(arguments, 0)
            let bundleURL = URL(fileURLWithPath: try requiredArgument(arguments, 1))
            let configuration = try DaemonLaunchConfiguration.current(bundleURL: bundleURL)
            let launcher = DaemonLauncher(
                configurationLoader: { configuration },
                commandRunner: run
            )

            switch action {
            case "prepare":
                let pid = Int32(try requiredArgument(arguments, 2))!
                let instance = try requiredArgument(arguments, 3)
                let executable = try requiredArgument(arguments, 4)
                let transaction = try await launcher.prepareRuntimeSwitch(
                    expectedPID: pid,
                    expectedInstanceId: instance,
                    expectedExecutable: executable
                )
                print("prepared \(transaction.journal.transactionId) candidate=\(transaction.journal.candidateBuild)")
            case "activate":
                let pid = Int32(try requiredArgument(arguments, 2))!
                let executable = try requiredArgument(arguments, 3)
                guard let transaction = try await launcher.loadPendingRuntimeSwitch() else {
                    throw HarnessError(message: "no pending runtime switch")
                }
                try await launcher.activatePreparedRuntime(
                    transaction,
                    expectedPID: pid,
                    expectedExecutable: executable
                )
                print("activated candidate=\(transaction.journal.candidateBuild)")
            case "commit":
                guard let transaction = try await launcher.loadPendingRuntimeSwitch() else {
                    throw HarnessError(message: "no pending runtime switch")
                }
                try await launcher.commitRuntimeSwitch(transaction)
                print("committed")
            case "rollback":
                guard let transaction = try await launcher.loadPendingRuntimeSwitch() else {
                    throw HarnessError(message: "no pending runtime switch")
                }
                try await launcher.rollbackRuntime(
                    transaction,
                    expectedPID: nil,
                    expectedExecutable: nil
                )
                try await launcher.commitRuntimeSwitch(transaction)
                print("rolled back")
            case "switch":
                let installationId = try requiredArgument(arguments, 2)
                let expectedInstanceId = try requiredArgument(arguments, 3)
                let expectedPID = Int32(try requiredArgument(arguments, 4))!
                let expectedExecutable = try requiredArgument(arguments, 5)
                let leaseGeneration = Int64(try requiredArgument(arguments, 6))!
                let client = try HTTPClient(
                    controlFile: URL(fileURLWithPath: try requiredArgument(arguments, 7))
                )
                let force = arguments.count > 8
                    ? (try requiredArgument(arguments, 8)) == "1"
                    : true
                let transaction = try await launcher.prepareRuntimeSwitch(
                    expectedPID: expectedPID,
                    expectedInstanceId: expectedInstanceId,
                    expectedExecutable: expectedExecutable
                )
                print("prepared candidate=\(transaction.journal.candidateBuild)")
                try await client.restart(
                    instanceId: expectedInstanceId,
                    installationId: installationId,
                    leaseGeneration: leaseGeneration,
                    force: force
                )
                var currentPID = expectedPID
                var activated = false
                for attempt in 0..<40 {
                    do {
                        try await launcher.activatePreparedRuntime(
                            transaction,
                            expectedPID: currentPID,
                            expectedExecutable: expectedExecutable
                        )
                        activated = true
                        print("activated candidate")
                        break
                    } catch {
                        guard attempt < 39 else { throw error }
                        if let current = try? await client.lifecycle(),
                           current.executable == expectedExecutable {
                            currentPID = Int32(current.service.pid)
                            if currentPID != expectedPID {
                                // A forced shutdown causes launchd to spawn a
                                // fresh old-runtime instance. The transactional
                                // candidate commit rebinds the same installation
                                // lease to the candidate, so activate this
                                // replacement directly instead of trying to
                                // claim a normal lease for it.
                            }
                        }
                        try await Task.sleep(for: .milliseconds(150))
                    }
                }
                guard activated else { throw HarnessError(message: "candidate activation did not complete") }

                var candidate: LifecyclePayload?
                for _ in 0..<120 {
                    if let current = try? await client.lifecycle(),
                       current.executable == transaction.journal.candidateProgramPath,
                       current.runtime.buildNumber == Int(transaction.journal.candidateBuild),
                       current.runtime.state == "draining"
                    {
                        candidate = current
                        break
                    }
                    try await Task.sleep(for: .milliseconds(250))
                }
                guard let candidate else {
                    throw HarnessError(message: "candidate did not pass lifecycle health checks")
                }
                print("candidate healthy pid=\(candidate.service.pid) instance=\(candidate.service.instanceId)")
                try await client.commit(
                    instanceId: candidate.service.instanceId,
                    installationId: installationId,
                    leaseGeneration: leaseGeneration
                )
                try await launcher.commitRuntimeSwitch(transaction)
                print("committed candidate")
            default:
                throw HarnessError(message: "unknown action \(action)")
            }
        } catch {
            fputs("force-switch: \(error.localizedDescription)\n", stderr)
            exit(1)
        }
    }
}
