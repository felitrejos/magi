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

    @Test func showsTheLastFiveMovesNewestFirst() async throws {
        defer { try? FileManager.default.removeItem(at: home) }
        let store = Store(home: home)
        let log = try FileHandle(forWritingTo: home.appending(path: "handoffs.jsonl"))
        let times = (1...6).map { Date(timeIntervalSince1970: TimeInterval($0)) }
        for time in times {
            try log.write(contentsOf: Data(#"{"time":"\#(time.formatted(.iso8601))","from":"laptop","to":"box","to_dir":"/home/sam/code/shop"}\#n"#.utf8))
            await until { store.moves.first?.time == time }
        }
        #expect(store.moves.map(\.time) == times.dropFirst().reversed())
    }

    @Test func keepsSixtySamplesTwentyFiveSecondsApartAndThirtyTwoStateChanges() async throws {
        defer { try? FileManager.default.removeItem(at: home) }
        let store = Store(home: home)
        let laptop = String(decoding: try fixture("live/laptop.json"), as: UTF8.self)
        let start = Date(timeIntervalSince1970: 1_800_000_000)
        for step in 0..<130 {
            let heard = start.addingTimeInterval(Double(step) * 15)
            let state = step.isMultiple(of: 2) ? "waiting" : "busy"
            let live = laptop.replacing("2026-11-20T17:45:33.936688+01:00", with: heard.formatted(.iso8601)).replacing(#""load":2.36"#, with: #""load":\#(step)"#).replacing(#""state":"waiting""#, with: #""state":"\#(state)""#)
            try publish(Data(live.utf8), as: "laptop")
            await until { store.machines["laptop"]?.load == Double(step) }
        }
        let load = try #require(store.load["laptop"])
        #expect(load.count == Store.samples)
        #expect(load.map(\.value) == stride(from: 10, through: 128, by: 2).map(Double.init))
        #expect(zip(load, load.dropFirst()).allSatisfy { $1.time.timeIntervalSince($0.time) >= Store.spacing })
        #expect(store.cpu["laptop/07f37932-4bda-4e5a-a489-719326b0f477"]?.count == Store.samples)
        #expect(store.states["laptop/07f37932-4bda-4e5a-a489-719326b0f477"]?.count == Store.transitions)
        #expect(store.states["laptop/07f37932-4bda-4e5a-a489-719326b0f477"]?.last?.state == "busy")
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
