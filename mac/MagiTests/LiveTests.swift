import Foundation
import SwiftUI
import Testing

func fixture(_ path: String) throws -> Data {
    try Data(contentsOf: URL(filePath: #filePath).deletingLastPathComponent().appending(path: "Fixtures/\(path)"))
}

struct LiveTests {
    @Test func readsALiveFileWithOrWithoutItsMonitoringFields() throws {
        let laptop = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/laptop.json"))
        let box = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/box.json"))
        #expect(laptop.answering)
        #expect(laptop.heard?.timeIntervalSince1970 == 1795193133.936688)
        #expect(laptop.cores == 10 && laptop.load == 2.36)
        #expect(laptop.memory.usedMB == 12567 && laptop.memory.totalMB == 16384)
        #expect(laptop.disk.usedMB == 281527 && laptop.disk.totalMB == 471482)
        let waiting = laptop.sessions[0]
        #expect(waiting.id == "07f37932-4bda-4e5a-a489-719326b0f477" && waiting.title == "Shop checkout fix")
        #expect(waiting.folder == "/Users/sam/code/shop" && waiting.state == "waiting" && waiting.waitingOn == "permission")
        #expect(waiting.since?.timeIntervalSince1970 == 1795193063)
        #expect(waiting.cpu == 0 && waiting.memoryMB == 229)
        #expect(laptop.sessions[1].waitingOn == nil)
        #expect(laptop.pressure == .warning)
        #expect(laptop.swap?.usedMB == 1536 && laptop.swap?.totalMB == 4096)
        #expect(laptop.bootedAt?.timeIntervalSince1970 == 1794989525)
        #expect(laptop.battery?.percent == 64 && laptop.battery?.onBattery == true)
        #expect(laptop.relayed == nil && box.relayed == true)
        #expect(box.pressure == nil && box.swap == nil && box.bootedAt == nil && box.battery == nil)
        #expect(laptop.history?.first.map { [$0.at, $0.load, Double($0.memoryMB)] } == [1795189563, 1.82, 12210])
        #expect(laptop.history?.count == 3 && laptop.sessions[1].cpuHistory?.map(\.cpu) == [4, 6])
        #expect(box.history == nil && box.sessions[0].cpuHistory == nil)
    }

    @Test func stopsAnsweringWhenQuietForFortySecondsOrSaysSo() throws {
        let laptop = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/laptop.json"))
        let box = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/box.json"))
        let heard = try #require(laptop.heard)
        #expect(laptop.answers(at: heard.addingTimeInterval(40)))
        #expect(!laptop.answers(at: heard.addingTimeInterval(40.001)))
        #expect(!box.answers(at: try #require(box.heard)))
    }

    @Test func callsADiskNearlyFullFromNinetyPercent() {
        #expect(!Space(usedMB: 899, totalMB: 1000).nearlyFull)
        #expect(Space(usedMB: 900, totalMB: 1000).nearlyFull)
    }

    @Test func coloursAMachineGreenUnderSeventyPercentLoadOrangeFromThereOrUnderPressureRedWhenCritical() {
        func machine(load: Double, pressure: Pressure? = nil, disk: Int = 100) -> Machine {
            Machine(answering: true, heard: nil, ssh: nil, cores: 10, load: load, memory: Space(usedMB: 1, totalMB: 2), disk: Space(usedMB: disk, totalMB: 1000), sessions: [], pressure: pressure, swap: nil, bootedAt: nil, battery: nil, relayed: nil, history: nil)
        }
        #expect(machine(load: 6.9).health == .green)
        #expect(machine(load: 7).health == .orange)
        #expect(machine(load: 1, pressure: .warning).health == .orange)
        #expect(machine(load: 1, disk: 900).health == .orange)
        #expect(machine(load: 12, pressure: .critical).health == .red)
    }

    @Test func putsWaitingFirstLongestWaitingFirstThenBusyThenIdleAndKeepsAFrozenOrder() {
        func row(_ machine: String, _ id: String, _ state: String, _ since: TimeInterval) -> Row {
            Row(machine: machine, session: Session(id: id, title: id, folder: "/", state: state, waitingOn: nil, since: Date(timeIntervalSince1970: since), cpu: 0, memoryMB: 0, cpuHistory: nil))
        }
        let rows = [row("a", "idle", "idle", 1), row("a", "busy", "busy", 2), row("a", "late", "waiting", 9), row("b", "early", "waiting", 3), row("b", "busy", "busy", 1)]
        let urgent = Row.urgent(rows)
        #expect(urgent.map(\.id) == ["b/early", "a/late", "a/busy", "b/busy", "a/idle"])
        let frozen = ["a/idle", "b/early", "gone/x"]
        #expect(Row.ordered(urgent, keeping: frozen).map(\.id) == ["a/idle", "b/early", "a/late", "a/busy", "b/busy"])
    }
}
