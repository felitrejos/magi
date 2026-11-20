import SwiftUI

struct PanelView: View {
    let store: Store
    let close: () -> Void
    @State private var selection: String?
    @FocusState private var focused: Bool

    private var machines: [(key: String, value: Machine)] {
        store.machines.sorted { $0.key < $1.key }
    }

    private var waiting: [(machine: String, session: Session)] {
        machines.flatMap { name, machine in machine.sessions.filter { $0.state == "waiting" }.map { (name, $0) } }
    }

    private var rows: [String] {
        waiting.map { "waiting \($0.session.id)" } + machines.flatMap { name, machine in machine.sessions.map { "\(name) \($0.id)" } }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if !waiting.isEmpty {
                    Text("Waiting on you").font(.headline)
                    ForEach(waiting, id: \.session.id) { machine, session in
                        VStack(alignment: .leading) {
                            Text("\(session.title) on \(machine)")
                            if let waitingOn = session.waitingOn {
                                Text(waitingOn)
                            }
                        }
                        .background(selection == "waiting \(session.id)" ? Color.accentColor.opacity(0.3) : .clear)
                    }
                }
                ForEach(machines, id: \.key) { name, machine in
                    VStack(alignment: .leading, spacing: 6) {
                        Text(name).font(.headline)
                        if store.answering.contains(name) {
                            Text("Load \(machine.load, format: .number) on \(machine.cores) cores, memory \(megabytes(machine.memory.usedMB)) of \(megabytes(machine.memory.totalMB)), disk \(megabytes(machine.disk.usedMB)) of \(megabytes(machine.disk.totalMB))")
                        } else if let heard = machine.heard {
                            Text("Not answering, last heard \(heard, style: .relative) ago")
                        } else {
                            Text("Not answering")
                        }
                        ForEach(machine.sessions) { session in
                            VStack(alignment: .leading) {
                                Text(session.title)
                                HStack {
                                    Text(URL(filePath: session.folder).lastPathComponent)
                                    Text(session.state)
                                    if let since = session.since {
                                        Text(since, style: .relative)
                                    }
                                    Text("CPU \(session.cpu)%")
                                    Text(megabytes(session.memoryMB))
                                }
                            }
                            .background(selection == "\(name) \(session.id)" ? Color.accentColor.opacity(0.3) : .clear)
                        }
                    }
                    .opacity(store.answering.contains(name) ? 1 : 0.5)
                }
                if !store.moves.isEmpty {
                    Text("Recent moves").font(.headline)
                    ForEach(store.moves, id: \.time) { move in
                        if let note = move.note {
                            Text(note)
                        } else {
                            Text("\(URL(filePath: move.toDir).lastPathComponent) from \(move.from) to \(move.to), \(move.time, style: .relative) ago")
                        }
                    }
                }
            }
            .padding()
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(width: 380, height: 520)
        .focusable()
        .focusEffectDisabled()
        .focused($focused)
        .onAppear { focused = true }
        .onMoveCommand(perform: move)
        .onExitCommand(perform: close)
    }

    private func move(_ direction: MoveCommandDirection) {
        guard direction == .up || direction == .down else { return }
        let rows = rows
        let current = selection.flatMap(rows.firstIndex(of:))
        let next = if direction == .down { (current ?? -1) + 1 } else { (current ?? rows.count) - 1 }
        if rows.indices.contains(next) {
            selection = rows[next]
        }
    }

    private func megabytes(_ value: Int) -> String {
        (Int64(value) << 20).formatted(.byteCount(style: .memory))
    }
}
