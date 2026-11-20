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
        try publish(fixture("live/laptop.json"), as: "laptop")
        await until { store.machines["laptop"]?.cores == 10 }
        let start = ContinuousClock.now
        try publish(fixture("live/box.json"), as: "laptop")
        await until { store.machines["laptop"]?.cores == 12 }
        let latency = ContinuousClock.now - start
        print("A live file change reached the store in \(latency.formatted(.units(allowed: [.milliseconds], fractionalPart: .show(length: 2))))")
        #expect(latency < .seconds(1))
    }

    @Test func stopsAnsweringAtTheDeadline() async throws {
        defer { try? FileManager.default.removeItem(at: home) }
        let store = Store(home: home)
        let heard = Date(timeIntervalSince1970: (Date.now.timeIntervalSince1970 - 39).rounded(.up))
        let live = String(decoding: try fixture("live/laptop.json"), as: UTF8.self).replacing("2026-11-20T17:45:33.936688+01:00", with: heard.formatted(.iso8601))
        try publish(Data(live.utf8), as: "laptop")
        await until { store.answering.contains("laptop") }
        await until { !store.answering.contains("laptop") }
        #expect(Date.now.timeIntervalSince(heard) >= Machine.quietAfter)
    }

    private func publish(_ data: Data, as name: String) throws {
        let live = home.appending(path: "live")
        let temp = live.appending(path: "\(name).json.1")
        try data.write(to: temp)
        #expect(rename(temp.path, live.appending(path: "\(name).json").path) == 0)
    }

    private func until(_ condition: @escaping @MainActor () -> Bool) async {
        for await met in Observations(condition) where met {
            return
        }
    }
}
