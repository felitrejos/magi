import Foundation
import Observation
import Testing

@MainActor
@Suite(.timeLimit(.minutes(1)))
struct StoreTests {
    let home = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)

    @Test func showsALiveFileChangeWithinASecond() async throws {
        defer { try? FileManager.default.removeItem(at: home) }
        let store = Store(home: home)
        try publish(fixture("live/laptop.json"), as: "laptop", in: home)
        await until { store.machines["laptop"]?.cores == 10 }
        let start = ContinuousClock.now
        try publish(fixture("live/box.json"), as: "laptop", in: home)
        await until { store.machines["laptop"]?.cores == 12 }
        let latency = ContinuousClock.now - start
        print("A live file change reached the store in \(latency.formatted(.units(allowed: [.milliseconds], fractionalPart: .show(length: 2))))")
        #expect(latency < .seconds(1))
    }

    @Test func keepsTheFirstOfEachSessionAMachineRepeats() async throws {
        defer { try? FileManager.default.removeItem(at: home) }
        let store = Store(home: home)
        var live = try #require(JSONSerialization.jsonObject(with: fixture("live/laptop.json")) as? [String: Any])
        let sessions = try #require(live["sessions"] as? [[String: Any]])
        live["sessions"] = [sessions[0], sessions[0].merging(["title": "again"]) { $1 }]
        try publish(JSONSerialization.data(withJSONObject: live), as: "laptop", in: home)
        await until { store.machines["laptop"] != nil }
        #expect(store.rows.map(\.session.title) == ["Shop checkout fix"])
        #expect(Glance(store: store).waiting.count == 1)
    }

    @Test func stopsAnsweringAtTheDeadline() async throws {
        defer { try? FileManager.default.removeItem(at: home) }
        let store = Store(home: home)
        let heard = Date(timeIntervalSince1970: (Date.now.timeIntervalSince1970 - 39).rounded(.up))
        let live = String(decoding: try fixture("live/laptop.json"), as: UTF8.self).replacing("2026-11-20T17:45:33.936688+01:00", with: heard.formatted(.iso8601))
        try publish(Data(live.utf8), as: "laptop", in: home)
        await until { store.answering.contains("laptop") }
        await until { !store.answering.contains("laptop") }
        #expect(Date.now.timeIntervalSince(heard) >= Machine.quietAfter)
    }
}

func publish(_ data: Data, as name: String, in home: URL) throws {
    let live = home.appending(path: "live")
    let temp = live.appending(path: "\(name).json.1")
    try data.write(to: temp)
    #expect(rename(temp.path, live.appending(path: "\(name).json").path) == 0)
}

@MainActor func until(_ condition: @escaping @MainActor () -> Bool) async {
    for await met in Observations(condition) where met {
        return
    }
}
