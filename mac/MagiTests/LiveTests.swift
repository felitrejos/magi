import Foundation
import Testing

func fixture(_ path: String) throws -> Data {
    try Data(contentsOf: URL(filePath: #filePath).deletingLastPathComponent().appending(path: "Fixtures/\(path)"))
}

struct LiveTests {
    @Test func readsALiveFile() throws {
        let machine = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/laptop.json"))
        #expect(machine.answering)
        #expect(machine.heard?.timeIntervalSince1970 == 1795193133.936688)
        #expect(machine.cores == 10 && machine.load == 2.36)
        #expect(machine.memory.usedMB == 12567 && machine.memory.totalMB == 16384)
        #expect(machine.disk.usedMB == 281527 && machine.disk.totalMB == 471482)
        let waiting = machine.sessions[0]
        #expect(waiting.id == "07f37932-4bda-4e5a-a489-719326b0f477" && waiting.title == "Shop checkout fix")
        #expect(waiting.folder == "/Users/sam/code/shop" && waiting.state == "waiting" && waiting.waitingOn == "permission")
        #expect(waiting.since?.timeIntervalSince1970 == 1795193063)
        #expect(waiting.cpu == 0 && waiting.memoryMB == 229)
        #expect(machine.sessions[1].waitingOn == nil)
    }

    @Test func readsTheMovesInHandoffs() throws {
        let moves = Move.recent(in: try fixture("handoffs.jsonl"))
        #expect(moves.count == 4)
        #expect(moves[3].note?.hasPrefix("Moving session 3fce93f1 to box was interrupted.") == true)
        #expect(moves[0].from == "laptop" && moves[0].to == "box" && moves[0].toDir == "/home/sam/code/shop" && moves[0].note == nil)
        #expect(abs(moves[0].time.timeIntervalSince1970 - 1795192998.75481) < 0.000_001)
    }

    @Test func stopsAnsweringWhenQuietForFortySecondsOrSaysSo() throws {
        let laptop = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/laptop.json"))
        let box = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/box.json"))
        let heard = try #require(laptop.heard)
        #expect(laptop.answers(at: heard.addingTimeInterval(40)))
        #expect(!laptop.answers(at: heard.addingTimeInterval(40.001)))
        #expect(!box.answers(at: try #require(box.heard)))
    }

    @Test func readsTheMonitoringFieldsAndToleratesTheirAbsence() throws {
        let laptop = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/laptop.json"))
        let box = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/box.json"))
        #expect(laptop.pressure == .warning)
        #expect(laptop.swap?.usedMB == 1536 && laptop.swap?.totalMB == 4096)
        #expect(laptop.bootedAt?.timeIntervalSince1970 == 1794989525)
        #expect(laptop.battery?.percent == 64 && laptop.battery?.charging == false && laptop.battery?.onBattery == true)
        #expect(laptop.relayed == nil && box.relayed == true)
        #expect(box.pressure == nil && box.swap == nil && box.bootedAt == nil && box.battery == nil)
    }

    @Test func measuresAMoveInMegabytesPerSecond() throws {
        let moves = Move.recent(in: try fixture("handoffs.jsonl"))
        #expect(moves[0].speed?.hasSuffix(" GB at 48 MB/s") == true)
        #expect(moves[1].speed == nil)
    }

    @Test func tintsMemoryByItsPressureAndDiskFromNinetyPercent() {
        #expect(Space(usedMB: 899, totalMB: 1000).pressure == .normal)
        #expect(Space(usedMB: 900, totalMB: 1000).pressure == .warning)
        #expect([Pressure.normal, .warning, .critical].map(\.tint) == [.secondary, .orange, .red])
    }

    @Test func putsWaitingFirstLongestWaitingFirstThenBusyThenIdleAndKeepsAFrozenOrder() {
        func row(_ machine: String, _ id: String, _ state: String, _ since: TimeInterval) -> Row {
            Row(machine: machine, session: Session(id: id, title: id, folder: "/", state: state, waitingOn: nil, since: Date(timeIntervalSince1970: since), cpu: 0, memoryMB: 0))
        }
        let rows = [row("a", "idle", "idle", 1), row("a", "busy", "busy", 2), row("a", "late", "waiting", 9), row("b", "early", "waiting", 3), row("b", "busy", "busy", 1)]
        let urgent = Row.urgent(rows)
        #expect(urgent.map(\.id) == ["b/early", "a/late", "a/busy", "b/busy", "a/idle"])
        let frozen = ["a/idle", "b/early", "gone/x"]
        #expect(Row.ordered(urgent, keeping: frozen).map(\.id) == ["a/idle", "b/early", "a/late", "a/busy", "b/busy"])
        #expect(Row.ordered(urgent, keeping: nil).map(\.id) == urgent.map(\.id))
    }
}
