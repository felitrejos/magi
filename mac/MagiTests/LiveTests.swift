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
        #expect(laptop.history?.first?.at == 1795189563)
        #expect(laptop.history?.count == 3 && laptop.sessions[1].cpuHistory?.map(\.cpu) == [4, 6])
        #expect(laptop.history?.last.map { [$0.cpu, $0.thermal, $0.rx, $0.tx] } == [31, 1, 2_400_000, 310_000] && laptop.history?.first?.cpu == nil)
        #expect(box.critical == 105 && laptop.critical == nil)
        #expect(box.history == nil && box.sessions[0].cpuHistory == nil)
        #expect(waiting.hostSession == "local_7cbf1d10" && waiting.app == "com.anthropic.claudefordesktop" && waiting.tmux == nil)
        #expect(laptop.sessions[1].tmux == "%3" && laptop.sessions[1].tty == "/dev/ttys007" && laptop.sessions[1].app == "com.mitchellh.ghostty")
        #expect(box.sessions[0].rex == RexBlock(session: "session:01a1", block: "block:01a2") && box.sessions[0].remoteControl?.absoluteString == "https://claude.ai/code/session_01AbCdEfGhIjKlMnOpQrStUv")
    }

    @Test func stopsAnsweringWhenQuietForFortySecondsOrSaysSo() throws {
        let laptop = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/laptop.json"))
        let box = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/box.json"))
        let heard = try #require(laptop.heard)
        #expect(laptop.answers(at: heard.addingTimeInterval(40)))
        #expect(!laptop.answers(at: heard.addingTimeInterval(40.001)))
        #expect(!box.answers(at: try #require(box.heard)))
    }

    @Test func measuresEachModuleWithDataAndColorsItByItsThresholds() {
        func machine(_ readings: [(cpu: Int?, temp: Double?, thermal: Int?)] = [], critical: Double? = nil, disk: Int = 100, pressure: Pressure? = nil) -> Machine {
            let history = readings.enumerated().map { Reading(at: Double($0.offset * 30), cpu: $0.element.cpu, temp: $0.element.temp, thermal: $0.element.thermal, rx: nil, tx: nil) }
            return Machine(answering: true, heard: nil, ssh: nil, cores: 10, load: 1, memory: Space(usedMB: 1, totalMB: 2), disk: Space(usedMB: disk, totalMB: 1000), sessions: [], pressure: pressure, swap: nil, bootedAt: nil, battery: nil, critical: critical, relayed: nil, history: history, top: nil, memoryParts: nil, volumes: nil, interfaces: nil, sensors: nil)
        }
        func health(_ machine: Machine, _ module: Module) -> Health? {
            machine.measures([module]).first?.health
        }
        #expect(machine().measures([.cpu, .temperature, .network]).isEmpty)
        #expect(machine().measures([.memory, .disk]).map(\.module) == [.memory, .disk])
        #expect([[74, 95], [75, 95], [90, 91], [95]].map { health(machine($0.map { ($0, nil, nil) }), .cpu) } == [.fine, .warning, .critical, .critical])
        #expect([79.9, 80, 90].map { health(machine([(nil, $0, nil)]), .temperature) } == [.fine, .warning, .critical])
        #expect([84, 85].map { health(machine([(nil, $0, nil)], critical: 95), .temperature) } == [.warning, .critical])
        #expect([0, 1, 2, 4].map { health(machine([(nil, nil, $0)]), .temperature) } == [.fine, .warning, .critical, .critical])
        #expect(machine([(nil, nil, 4)]).measures([.temperature]).first?.value == "Critical")
        #expect([899, 900, 950].map { health(machine(disk: $0), .disk) } == [.fine, .warning, .critical])
        #expect([Pressure.normal, .warning, .critical].map { health(machine(pressure: $0), .memory) } == [.fine, .warning, .critical])
        let strained = machine([(nil, 85, nil)], disk: 960, pressure: .critical)
        #expect(strained.alert([.temperature, .disk, .memory])?.alert == "Disk full")
        #expect(strained.alert([.memory, .disk])?.alert == "Out of memory")
    }

    @Test func plotsOneReadingAsAFlatLineOverAtLeastAMinute() {
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        let one = Plot([Point(at: now, value: 5)], now: now)
        #expect(one.start == now - 60 && one.end == now && one.points.map(\.at) == [now - 60, now] && one.points.map(\.value) == [5, 5])
        let short = Plot([Point(at: now - 20, value: 1), Point(at: now - 10, value: 2)], now: now)
        #expect(short.start == now - 60 && short.end == now && short.points.map(\.value) == [1, 2])
    }

    @Test func storesTheEnabledModulesInOrderAndSkipsUnknownOnes() {
        #expect(Modules(rawValue: "disk,gpu,battery,cpu").enabled == [.disk, .cpu])
    }

    @Test func putsWaitingFirstLongestWaitingFirstThenBusyThenIdleAndKeepsAFrozenOrder() {
        func row(_ machine: String, _ id: String, _ state: String, _ since: TimeInterval) -> Row {
            Row(machine: machine, remote: nil, session: session(id, state: state, since: since))
        }
        let rows = [row("a", "idle", "idle", 1), row("a", "busy", "busy", 2), row("a", "late", "waiting", 9), row("b", "early", "waiting", 3), row("b", "busy", "busy", 1)]
        let urgent = Row.urgent(rows)
        #expect(urgent.map(\.id) == ["b/early", "a/late", "a/busy", "b/busy", "a/idle"])
        let frozen = ["a/idle", "b/early", "gone/x"]
        #expect(urgent.ordered(keeping: frozen).map(\.id) == ["a/idle", "b/early", "a/late", "a/busy", "b/busy"])
    }
}

func session(_ id: String = "a", state: String = "idle", since: TimeInterval = 0, hostSession: String? = nil, tmux: String? = nil, rex: RexBlock? = nil, app: String? = nil, tty: String? = nil, remoteControl: URL? = nil) -> Session {
    Session(id: id, title: id, folder: "/", state: state, waitingOn: nil, since: Date(timeIntervalSince1970: since), cpu: 0, memoryMB: 0, cpuHistory: nil, remoteControl: remoteControl, hostSession: hostSession, tmux: tmux, rex: rex, app: app, tty: tty)
}
