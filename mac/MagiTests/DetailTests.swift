import Foundation
import Testing

struct DetailTests {
    let homelab: Machine
    let laptop: Machine

    init() throws {
        homelab = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/homelab.json"))
        laptop = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/laptop.json"))
    }

    @Test func readsTheDetailFieldsOrAnOlderWatcherWithoutThem() {
        #expect(homelab.top?.cpu?.first.map { "\($0.pid) \($0.name) \($0.cpu) \($0.session ?? "")" } == "4121 claude 41 660b4a59-d508-4dec-8af1-9a047faa9dc6")
        #expect(homelab.top?.memory?.first.map { "\($0.pid) \($0.name) \($0.memoryMB) \($0.session ?? "none")" } == "811 Google Chrome 4900 none")
        #expect(homelab.memoryParts?.first.map { "\($0.kind) \($0.mb)" } == "app 14100")
        #expect(homelab.volumes?.first.map { "\($0.name) \($0.mount) \($0.usedMB) \($0.totalMB)" } == "Macintosh HD / 512000 994000")
        #expect(homelab.interfaces?.first.map { "\($0.name) \($0.rx) \($0.tx)" } == "en0 1600000 310000")
        #expect(homelab.sensors?.map { "\($0.name) \($0.temp) \($0.critical ?? 0)" } == ["Package id 0 51.0 100.0", "nvme 42.5 0.0"])
        #expect(laptop.top == nil && laptop.memoryParts == nil && laptop.volumes == nil && laptop.interfaces == nil && laptop.sensors == nil)
    }

    @Test func opensADetailOnlyFromACardWithItsData() throws {
        #expect(Module.allCases.filter(homelab.hasDetail) == Module.allCases && Module.allCases.filter { homelab.detail($0, on: "homelab") != nil } == Module.allCases)
        #expect(Module.allCases.filter(laptop.hasDetail).isEmpty)
        let firstSample = try machine {
            $0["top"] = ["cpu": [Any]()]
            $0["sensors"] = nil
        }
        #expect(Module.allCases.filter(firstSample.hasDetail) == [.disk, .network])
    }

    @Test func drawsTheKnownMemoryPartsInTheirOrder() throws {
        #expect(homelab.parts.map(\.kind) == MemoryKind.allCases)
        let linux = try machine { $0["memoryParts"] = [["kind": "free", "mb": 2], ["kind": "buffers", "mb": 9], ["kind": "cached", "mb": 3], ["kind": "app", "mb": 1]] }
        #expect(linux.parts.map(\.kind) == [.app, .cached, .free] && linux.parts.map(\.mb) == [1, 3, 2])
    }

    @Test func measuresEachRowOnTheWholeMachineScale() throws {
        let cpu = try #require(homelab.detail(.cpu, on: "homelab"))
        #expect(cpu.entries.map(\.fraction) == [0.41, 2.5] && cpu.entries.map(\.figures) == [["41%"], ["250%"]] && cpu.columns == [["999%"]])
        #expect(cpu.entries.map(\.spoken) == ["41 percent of one core", "250 percent of one core"])
        #expect(cpu.entries.map { $0.row?.session.title } == ["Upgrade Postgres to 17", nil] && cpu.entries[0].row?.route(rexInstalled: false) == .attach(pane: "%7", ssh: "homelab"))

        let memory = try #require(homelab.detail(.memory, on: "homelab"))
        #expect(memory.entries.map(\.fraction) == [4900 / 36000.0, 820 / 36000.0] && memory.entries.map(\.figures) == [["\(tenths(4900 / 1024.0)) GB"], ["820 MB"]])
        #expect(memory.scale == amount(homelab.memory).unit && memory.columns == [["999 MB", "\(tenths(36000 / 1024.0)) GB"]])

        let disk = try #require(homelab.detail(.disk, on: "homelab"))
        #expect(disk.entries.map(\.fraction) == [512_000 / 994_000.0, 0.95] && disk.entries.map(\.health) == [.fine, .critical] && disk.entries[1].spoken.hasSuffix(" free, full"))

        let network = try #require(homelab.detail(.network, on: "homelab"))
        #expect(network.entries.map(\.fraction) == [1, 382_000 / 1_910_000.0] && network.entries[1].figures == ["\u{2193} \(rate(382_000))", "\u{2191} \(rate(0))"])

        let temperature = try #require(homelab.detail(.temperature, on: "homelab"))
        #expect(temperature.entries.map(\.fraction) == [0.51, 0.425] && temperature.entries.map(\.figures) == [["51 \u{00B0}C"], ["43 \u{00B0}C"]] && temperature.entries.map(\.id) == ["Package id 0", "nvme"])
        let hot = try #require(try machine { $0["sensors"] = [["name": "nvme", "temp": 80.0, "critical": 84.85]] }.detail(.temperature, on: "homelab"))
        #expect(hot.entries[0].health == .critical && hot.entries[0].spoken == "80 degrees Celsius, critical at 84 degrees, critical")
    }

    @Test func warnsTenDegreesBeforeASensorRunsHot() {
        #expect([64.84, 64.85, 74.85].map { Health(celsius: $0, critical: 84.85) } == [.fine, .warning, .critical])
        #expect([79.9, 80, 90].map { Health(celsius: $0, critical: 100) } == [.fine, .warning, .critical])
        #expect([79.9, 80, 90].map { Health(celsius: $0, critical: nil) } == [.fine, .warning, .critical])
    }

    @Test func printsRatesAndSizesWithAStableWidth() {
        #expect([0, 400, 310_000, 999_499, 999_500, 1_640_000, 999_400_000, 1_234_000_000].map { rate(Double($0)) } == ["0 kB/s", "\(figure(0.4)) kB/s", "310 kB/s", "999 kB/s", "1 MB/s", "\(figure(1.6)) MB/s", "999 MB/s", "\(figure(1.2)) GB/s"])
        #expect([160, 999, 1000, 4900].map { amount(megabytes: $0) } == ["160 MB", "999 MB", "\(tenths(1000 / 1024.0)) GB", "\(tenths(4900 / 1024.0)) GB"])
    }

    @Test func stepsBackOneLevelAndKeepsTheDetailOnMachinesWithItsData() {
        let machines = ["homelab": homelab, "lab": homelab, "laptop": laptop]
        var place = Place(machine: "homelab", module: .cpu)
        place.show("lab", in: machines)
        #expect(place == Place(machine: "lab", module: .cpu))
        place.show("laptop", in: machines)
        #expect(place == Place(machine: "laptop"))
        place = Place(machine: "lab", module: .network)
        place.show(nil, in: machines)
        #expect(place == Place())
        #expect(Place(machine: "lab", module: .network).back == Place(machine: "lab"))
        #expect(Place(machine: "lab").back == Place() && Place().back == nil)
        #expect(Place(machine: "lab", module: .cpu).settled(in: ["lab": laptop]) == Place(machine: "lab"))
        #expect(Place(machine: "lab", module: .cpu).settled(in: machines) == Place(machine: "lab", module: .cpu))
    }

    private func machine(_ change: (inout [String: Any]) -> Void) throws -> Machine {
        var live = try #require(JSONSerialization.jsonObject(with: fixture("live/homelab.json")) as? [String: Any])
        change(&live)
        return try JSONDecoder.magi.decode(Machine.self, from: JSONSerialization.data(withJSONObject: live))
    }
}
