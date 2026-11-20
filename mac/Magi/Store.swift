import Foundation
import Observation

struct Sample {
    let time: Date
    let value: Double
}

struct Transition {
    let state: String
    let time: Date
}

@MainActor
@Observable
final class Store {
    static let samples = 60
    static let transitions = 32
    static let spacing: TimeInterval = 25

    private(set) var machines: [String: Machine] = [:]
    private(set) var answering: Set<String> = []
    private(set) var moves: [Move] = []
    private(set) var load: [String: [Sample]] = [:]
    private(set) var cpu: [String: [Sample]] = [:]
    private(set) var states: [String: [Transition]] = [:]

    @ObservationIgnored private let live: URL
    @ObservationIgnored private let log: URL
    @ObservationIgnored private var watches: [any DispatchSourceFileSystemObject] = []
    @ObservationIgnored private let deadline = DispatchSource.makeTimerSource(queue: .main)

    init(home: URL) {
        live = home.appending(path: "live")
        log = home.appending(path: "handoffs.jsonl")
        try? FileManager.default.createDirectory(at: live, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        watches = [watch(live, flags: O_EVTONLY) { $0.readLive() }, watch(log, flags: O_EVTONLY | O_CREAT) { $0.readLog() }]
        deadline.setEventHandler { [weak self] in MainActor.assumeIsolated { self?.evaluate() } }
        deadline.activate()
        readLive()
        readLog()
    }

    var rows: [Row] {
        Row.urgent(machines.flatMap { name, machine in machine.sessions.map { Row(machine: name, session: $0) } })
    }

    private func watch(_ url: URL, flags: Int32, read: @escaping @MainActor (Store) -> Void) -> any DispatchSourceFileSystemObject {
        let source = DispatchSource.makeFileSystemObjectSource(fileDescriptor: open(url.path, flags, 0o600), eventMask: [.write, .extend], queue: .main)
        source.setEventHandler { [weak self] in MainActor.assumeIsolated { self.map(read) } }
        source.activate()
        return source
    }

    private func readLive() {
        guard let files = try? FileManager.default.contentsOfDirectory(at: live, includingPropertiesForKeys: []) else { return }
        machines = files.filter { $0.pathExtension == "json" }.reduce(into: [:]) { machines, file in
            machines[file.deletingPathExtension().lastPathComponent] = try? JSONDecoder.magi.decode(Machine.self, from: Data(contentsOf: file))
        }
        record()
        evaluate()
    }

    private func readLog() {
        guard let data = try? Data(contentsOf: log) else { return }
        moves = Move.recent(in: data)
    }

    private func record() {
        var load: [String: [Sample]] = [:]
        var cpu: [String: [Sample]] = [:]
        var states: [String: [Transition]] = [:]
        for (name, machine) in machines {
            guard let heard = machine.heard else { continue }
            load[name] = appending(Sample(time: heard, value: machine.load), to: self.load[name])
            for row in machine.sessions.map({ Row(machine: name, session: $0) }) {
                cpu[row.id] = appending(Sample(time: heard, value: Double(row.session.cpu)), to: self.cpu[row.id])
                let past = self.states[row.id] ?? []
                states[row.id] = past.last?.state == row.session.state ? past : Array((past + [Transition(state: row.session.state, time: row.session.since ?? heard)]).suffix(Store.transitions))
            }
        }
        (self.load, self.cpu, self.states) = (load, cpu, states)
    }

    private func appending(_ sample: Sample, to samples: [Sample]?) -> [Sample] {
        guard let samples, let last = samples.last else { return [sample] }
        guard sample.time.timeIntervalSince(last.time) >= Store.spacing else { return samples }
        return Array((samples + [sample]).suffix(Store.samples))
    }

    private func evaluate() {
        let now = Date()
        let heard = machines.filter { $0.value.answers(at: now) }
        answering = Set(heard.keys)
        let next: DispatchWallTime? = heard.values.compactMap(\.heard).min().map { .now() + $0.addingTimeInterval(Machine.quietAfter).timeIntervalSince(now) }
        deadline.schedule(wallDeadline: next ?? .distantFuture)
    }
}
