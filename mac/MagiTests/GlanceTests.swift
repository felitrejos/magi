import Foundation
import Testing

@MainActor
@Suite(.timeLimit(.minutes(1)))
struct GlanceTests {
    let home = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let id = "laptop/07f37932-4bda-4e5a-a489-719326b0f477"

    @Test func notifiesOncePerChangeToWaitingAndWithdrawsItWhenTheWaitEnds() async throws {
        defer { try? FileManager.default.removeItem(at: home) }
        let store = Store(home: home)
        let launch = try await glance(store) { _ in }
        #expect(launch.started(after: nil).isEmpty)

        let idle = try await glance(store) { $0[0]["state"] = "idle" }
        #expect(idle.started(after: launch).isEmpty && idle.stopped(after: launch) == [id])

        let waiting = try await glance(store) { $0[0]["waitingOn"] = "Allow Bash to run npm test?" }
        let started = waiting.started(after: idle)
        #expect(Array(started.keys) == [id] && waiting.stopped(after: idle).isEmpty)
        let content = try #require(started[id]?.content)
        #expect([content.title, content.subtitle, content.body] == ["Shop checkout fix", "laptop \u{00B7} shop", "Allow Bash to run npm test?"])

        let still = try await glance(store) { _ in }
        #expect(still.started(after: waiting).isEmpty && still.stopped(after: waiting).isEmpty)

        let gone = try await glance(store) { $0.removeFirst() }
        #expect(gone.started(after: still).isEmpty && gone.stopped(after: still) == [id])
    }

    private func glance(_ store: Store, _ change: (inout [[String: Any]]) -> Void) async throws -> Glance {
        var machine = try #require(JSONSerialization.jsonObject(with: fixture("live/laptop.json")) as? [String: Any])
        var sessions = try #require(machine["sessions"] as? [[String: Any]])
        change(&sessions)
        machine["sessions"] = sessions
        let before = store.machines.isEmpty ? nil : Glance(store: store)
        try publish(JSONSerialization.data(withJSONObject: machine), as: "laptop", in: home)
        await until { !store.machines.isEmpty && Glance(store: store) != before }
        return Glance(store: store)
    }
}
