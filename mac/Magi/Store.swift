import Foundation
import Observation

@MainActor
@Observable
final class Store {
    private(set) var machines: [String: Machine] = [:]
    private(set) var answering: Set<String> = []
    private(set) var moves: [Move] = []

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
        evaluate()
    }

    private func readLog() {
        guard let data = try? Data(contentsOf: log) else { return }
        moves = Move.recent(in: data)
    }

    private func evaluate() {
        let now = Date()
        let heard = machines.filter { $0.value.answers(at: now) }
        answering = Set(heard.keys)
        let next: DispatchWallTime? = heard.values.compactMap(\.heard).min().map { .now() + $0.addingTimeInterval(Machine.quietAfter).timeIntervalSince(now) }
        deadline.schedule(wallDeadline: next ?? .distantFuture)
    }
}
