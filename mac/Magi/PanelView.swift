import SwiftUI

struct PanelView: View {
    static let radius: CGFloat = 16

    let store: Store
    let close: () -> Void
    @State private var scope: String?
    @State private var highlight: String?
    @State private var hovering = false
    @State private var order: [String]?
    @FocusState private var focused: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var names: [String] {
        store.machines.keys.sorted()
    }

    private var rows: [Row] {
        Row.ordered(store.rows.filter { scope == nil || $0.machine == scope }, keeping: order)
    }

    private var waiting: Int {
        store.rows.count { $0.session.state == "waiting" }
    }

    private var status: String {
        if let scope { return scope }
        if store.machines.isEmpty { return "magi isn't set up here" }
        if waiting > 0 { return "\(waiting) waiting on you" }
        if store.answering.isEmpty { return "No machine is answering" }
        let busy = store.rows.count { $0.session.state == "busy" }
        return busy > 0 ? "\(busy) working" : "All quiet"
    }

    var body: some View {
        TimelineView(.everyMinute) { context in
            VStack(spacing: 0) {
                header(now: context.date)
                if store.machines.isEmpty {
                    Text("Ask Claude to \u{201C}set up magi with my homelab\u{201D}, and your machines and sessions show here.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .padding([.horizontal, .bottom], 16)
                        .frame(maxWidth: .infinity, alignment: .leading)
                } else if !rows.isEmpty || scope != nil {
                    list(now: context.date)
                }
                if names.count > 1 {
                    strip(now: context.date)
                }
            }
        }
        .frame(width: 360)
        .containerShape(.rect(cornerRadius: PanelView.radius))
        .animation(reduceMotion ? .easeInOut(duration: 0.2) : .smooth(duration: 0.35), value: scope)
        .focusable()
        .focusEffectDisabled()
        .focused($focused)
        .onAppear { focused = true }
        .onMoveCommand(perform: move)
        .onExitCommand { scope == nil ? close() : (scope = nil) }
        .task(id: hovering || highlight != nil) {
            guard !hovering, highlight == nil else { return order = order ?? rows.map(\.id) }
            try? await Task.sleep(for: .seconds(1))
            order = nil
        }
    }

    private func header(now: Date) -> some View {
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 0) {
                Text(status)
                    .font(.title3.weight(.semibold))
                    .contentTransition(reduceMotion ? .opacity : .numericText(value: Double(waiting)))
                    .animation(.snappy(duration: 0.35), value: status)
                if let scope, let machine = store.machines[scope] {
                    ScopeCaption(machine: machine)
                }
            }
            Spacer(minLength: 0)
            if names.count == 1, let name = names.first, let machine = store.machines[name] {
                tile(name, machine, now: now)
                    .frame(width: 150)
            }
        }
        .padding(.horizontal, 16)
        .frame(minHeight: 44)
        .padding(.vertical, scope == nil ? 0 : 4)
    }

    private func list(now: Date) -> some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(spacing: 0) {
                    if let scope, let machine = store.machines[scope] {
                        ScopeSummary(name: scope, machine: machine, samples: store.load[scope] ?? [], now: now)
                            .padding(.bottom, 8)
                            .transition(.opacity)
                    }
                    ForEach(rows) { row in
                        SessionRow(row: row, showsMachine: scope == nil && names.count > 1, highlighted: highlight == row.id, history: scope == nil ? nil : History(cpu: store.cpu[row.id] ?? [], states: store.states[row.id] ?? [], now: now), now: now)
                            .id(row.id)
                            .onHover { inside in
                                if inside {
                                    highlight = row.id
                                } else if highlight == row.id {
                                    highlight = nil
                                }
                            }
                    }
                    if let scope {
                        Moves(moves: store.moves.filter { $0.from == scope || $0.to == scope })
                            .padding(.horizontal, 8)
                            .padding(.top, 8)
                    }
                }
                .padding(.horizontal, 8)
                .padding(.bottom, 8)
                .animation(reduceMotion ? nil : .snappy(duration: 0.35), value: rows.map(\.id))
                .animation(.easeOut(duration: 0.15), value: highlight)
            }
            .scrollBounceBehavior(.basedOnSize)
            .scrollEdgeEffectStyle(.hard, for: .bottom)
            .onHover { hovering = $0 }
            .onChange(of: highlight) {
                if let highlight {
                    proxy.scrollTo(highlight)
                }
            }
        }
    }

    private func strip(now: Date) -> some View {
        HStack(spacing: 6) {
            ForEach(names, id: \.self) { name in
                if let machine = store.machines[name] {
                    tile(name, machine, now: now)
                }
            }
        }
        .padding(8)
    }

    private func tile(_ name: String, _ machine: Machine, now: Date) -> some View {
        Button {
            scope = scope == name ? nil : name
            highlight = nil
        } label: {
            Tile(name: name, machine: machine, answers: store.answering.contains(name), selected: scope == name, now: now)
        }
        .buttonStyle(.plain)
    }

    private func move(_ direction: MoveCommandDirection) {
        if direction == .left || direction == .right {
            let index = scope.flatMap(names.firstIndex(of:))
            let next = direction == .right ? (index ?? -1) + 1 : (index ?? names.count) - 1
            scope = names.indices.contains(next) ? names[next] : nil
            highlight = nil
            return
        }
        let ids = rows.map(\.id)
        let current = highlight.flatMap(ids.firstIndex(of:))
        let next = if direction == .down { (current ?? -1) + 1 } else { (current ?? ids.count) - 1 }
        if ids.indices.contains(next) {
            highlight = ids[next]
        }
    }
}

struct SessionRow: View {
    let row: Row
    let showsMachine: Bool
    let highlighted: Bool
    let history: History?
    let now: Date

    private var waiting: Bool {
        row.session.state == "waiting"
    }

    private var folder: String {
        URL(filePath: row.session.folder).lastPathComponent
    }

    private var elapsed: String {
        guard let since = row.session.since, now.timeIntervalSince(since) >= 60 else { return "now" }
        return span(now.timeIntervalSince(since))
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            StateSymbol(state: row.session.state)
            VStack(alignment: .leading, spacing: 1) {
                Text(row.session.title)
                    .font(.body.weight(waiting ? .semibold : .regular))
                Text(showsMachine ? "\(folder) \u{00B7} \(row.machine)" : folder)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                if waiting, let question = row.session.waitingOn {
                    Text(question)
                        .font(.callout)
                        .lineLimit(2)
                        .truncationMode(.tail)
                        .padding(.top, 2)
                }
            }
            .lineLimit(1)
            .truncationMode(.middle)
            .frame(maxWidth: .infinity, alignment: .leading)
            if highlighted {
                HStack(spacing: 2) {
                    Affordance(label: "Open in Claude") {
                        Image(.claude)
                            .resizable()
                            .frame(width: 13, height: 13)
                    }
                    Affordance(label: "Open in a terminal") {
                        Image(systemName: "terminal")
                    }
                }
                .alignmentGuide(.firstTextBaseline) { $0[VerticalAlignment.center] + 4 }
                .transition(.opacity)
            }
            if let history {
                Usage(session: row.session, history: history)
            } else {
                Text(elapsed)
                    .font(.subheadline)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .frame(minHeight: waiting && row.session.waitingOn != nil ? 62 : 44)
        .background {
            if highlighted {
                ConcentricRectangle(corners: .concentric(minimum: 8), isUniform: true)
                    .fill(.quaternary)
            }
        }
        .contentShape(.rect)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(accessibility)
    }

    private var accessibility: String {
        let state = waiting ? "waiting on you" + (row.session.waitingOn.map { ": \($0)" } ?? "") : row.session.state
        return "\(row.session.title), \(state), \(folder) on \(row.machine), for \(elapsed)"
    }
}

struct StateSymbol: View {
    let state: String
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Group {
            switch state {
            case "waiting":
                Image(systemName: "hand.raised.fill").foregroundStyle(.tint)
            case "busy":
                TimelineView(.animation(minimumInterval: 1.0 / 12, paused: reduceMotion)) { context in
                    Image(systemName: "progress.indicator", variableValue: 0.7)
                        .foregroundStyle(.secondary)
                        .rotationEffect(.degrees((context.date.timeIntervalSinceReferenceDate * 12).rounded(.down).truncatingRemainder(dividingBy: 8) * 45))
                }
            default:
                Image(systemName: "circle").foregroundStyle(.tertiary)
            }
        }
        .font(.system(size: 13, weight: .medium))
        .frame(width: 16, height: 16)
        .accessibilityHidden(true)
    }
}

struct Affordance<Icon: View>: View {
    let label: String
    @ViewBuilder let icon: Icon
    @Environment(\.accessibilityShowBorders) private var showBorders

    var body: some View {
        Button {} label: {
            icon
                .font(.system(size: 12))
                .frame(width: 22, height: 22)
                .overlay {
                    if showBorders {
                        ConcentricRectangle(corners: .concentric(minimum: 5), isUniform: true).stroke(.secondary)
                    }
                }
                .contentShape(.rect)
        }
        .buttonStyle(.borderless)
        .help(label)
        .accessibilityLabel(label)
    }
}

struct Tile: View {
    let name: String
    let machine: Machine
    let answers: Bool
    let selected: Bool
    let now: Date
    @Environment(\.colorSchemeContrast) private var contrast
    @Environment(\.accessibilityShowBorders) private var showBorders

    var body: some View {
        HStack(spacing: 4) {
            VStack(alignment: .leading, spacing: 1) {
                Label {
                    Text(name)
                        .font(.subheadline.weight(.semibold))
                        .truncationMode(.middle)
                } icon: {
                    Image(systemName: machine.device)
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                }
                .labelStyle(.titleAndIcon)
                if let caption {
                    Text(caption)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
            .lineLimit(1)
            .opacity(answers ? 1 : 0.5)
            Spacer(minLength: 0)
            VStack(spacing: 3) {
                if let battery = machine.battery, battery.onBattery {
                    Image(systemName: "battery.\(min(battery.percent / 25 * 25, 100))percent")
                        .font(.system(size: 9))
                        .foregroundStyle(.secondary)
                        .accessibilityLabel("On battery, \(battery.percent)%")
                }
                Ring(fraction: machine.load / Double(max(machine.cores, 1)), dashed: !answers)
                    .accessibilityLabel(answers ? "Load \(machine.load.formatted()) on \(machine.cores) cores" : "Not answering")
            }
        }
        .padding(.horizontal, 8)
        .frame(height: 44)
        .background(ConcentricRectangle(corners: .concentric(minimum: 8), isUniform: true).fill(selected ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.quaternary)))
        .overlay {
            if contrast == .increased || showBorders {
                ConcentricRectangle(corners: .concentric(minimum: 8), isUniform: true).stroke(.separator)
            }
        }
        .contentShape(.rect)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private var caption: String? {
        if !answers, let heard = machine.heard {
            return "heard \(span(now.timeIntervalSince(heard))) ago"
        }
        if let booted = machine.bootedAt, now.timeIntervalSince(booted) < 15 * 60 {
            return "booted \(span(now.timeIntervalSince(booted))) ago"
        }
        return machine.relayed == true ? "relayed" : nil
    }
}

struct Ring: View {
    let fraction: Double
    let dashed: Bool
    @Environment(\.colorSchemeContrast) private var contrast

    var body: some View {
        let width: CGFloat = contrast == .increased ? 3 : 2
        ZStack {
            Circle()
                .stroke(dashed ? AnyShapeStyle(.secondary) : AnyShapeStyle(.quaternary), style: StrokeStyle(lineWidth: width, dash: dashed ? [2.4, 2.3] : []))
            if !dashed {
                Circle()
                    .trim(from: 0, to: min(fraction, 1))
                    .stroke(fraction > 1 ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary), style: StrokeStyle(lineWidth: width, lineCap: .round))
                    .rotationEffect(.degrees(-90))
            }
        }
        .frame(width: 12, height: 12)
    }
}

func span(_ seconds: TimeInterval) -> String {
    Duration.seconds(max(seconds, 60)).formatted(.units(allowed: [.days, .hours, .minutes], width: .narrow, maximumUnitCount: 1))
}

extension Machine {
    var device: String {
        if battery != nil { return "macbook" }
        return ssh?.isEmpty ?? true ? "desktopcomputer" : "server.rack"
    }
}
