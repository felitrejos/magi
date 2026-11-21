import Foundation
import Observation

@MainActor
@Observable
final class Store {
    private(set) var machines: [String: Machine] = [:]
    private(set) var rows: [Row] = []
    private(set) var answering: Set<String> = []

    @ObservationIgnored private let live: URL
    @ObservationIgnored private let watch: any DispatchSourceFileSystemObject
    @ObservationIgnored private let deadline = DispatchSource.makeTimerSource(queue: .main)
    @ObservationIgnored private var modified: [String: Date] = [:]

    init(home: URL) {
        live = home.appending(path: "live")
        try? FileManager.default.createDirectory(at: live, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        watch = DispatchSource.makeFileSystemObjectSource(fileDescriptor: open(live.path, O_EVTONLY), eventMask: [.write, .extend], queue: .main)
        watch.setEventHandler { [weak self] in MainActor.assumeIsolated { self?.read() } }
        watch.activate()
        deadline.setEventHandler { [weak self] in MainActor.assumeIsolated { self?.evaluate() } }
        deadline.activate()
        read()
    }

    private func read() {
        guard let files = try? FileManager.default.contentsOfDirectory(at: live, includingPropertiesForKeys: [.contentModificationDateKey]) else { return }
        let dated = files.filter { $0.pathExtension == "json" }.reduce(into: [String: (URL, Date)]()) { dated, file in
            dated[file.deletingPathExtension().lastPathComponent] = (file, (try? file.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast)
        }
        guard dated.mapValues(\.1) != modified else { return }
        machines = dated.reduce(into: [:]) { machines, entry in
            let (name, (file, date)) = entry
            machines[name] = modified[name] == date ? self.machines[name] : try? JSONDecoder.magi.decode(Machine.self, from: Data(contentsOf: file))
        }
        modified = dated.mapValues(\.1)
        rows = Row.urgent(machines.flatMap { name, machine in machine.sessions.map { Row(machine: name, remote: machine.ssh?.isEmpty == false ? machine.ssh : nil, session: $0) } })
        evaluate()
    }

    private func evaluate() {
        let now = Date()
        let heard = machines.filter { $0.value.answers(at: now) }
        answering = Set(heard.keys)
        let next: DispatchWallTime? = heard.values.compactMap(\.heard).min().map { .now() + $0.addingTimeInterval(Machine.quietAfter).timeIntervalSince(now) }
        deadline.schedule(wallDeadline: next ?? .distantFuture)
    }
}
