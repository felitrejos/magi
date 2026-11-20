import SwiftUI

struct PanelView: View {
    static let radius: CGFloat = 16

    let store: Store
    let close: () -> Void
    @State private var scope: String?
    @State private var highlight: String?
    @State private var hovering = false
    @State private var order: [String]?
    @State private var shown = false
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
        .scaleEffect(shown || reduceMotion ? 1 : 0.96, anchor: .top)
        .blur(radius: shown || reduceMotion ? 0 : 6)
        .opacity(shown ? 1 : 0)
        .containerShape(.rect(cornerRadius: PanelView.radius))
        .animation(reduceMotion ? nil : .smooth(duration: 0.4), value: scope)
        .focusable()
        .focusEffectDisabled()
        .focused($focused)
        .onAppear {
            focused = true
            withAnimation(reduceMotion ? .easeOut(duration: 0.15) : .spring(duration: 0.3, bounce: 0.15)) {
                shown = true
            }
        }
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
                tile(name, machine, order: 0, now: now)
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
                            .transition(.blurReplace)
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
            ForEach(Array(names.enumerated()), id: \.element) { order, name in
                if let machine = store.machines[name] {
                    tile(name, machine, order: order, now: now)
                }
            }
        }
        .padding(8)
    }

    private func tile(_ name: String, _ machine: Machine, order: Int, now: Date) -> some View {
        Button {
            scope = scope == name ? nil : name
            highlight = nil
        } label: {
            Tile(name: name, machine: machine, answers: store.answering.contains(name), selected: scope == name, delay: 0.08 + 0.03 * Double(order), now: now)
        }
        .buttonStyle(Press())
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
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

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
                    .contentTransition(reduceMotion ? .opacity : .numericText())
                    .animation(.snappy(duration: 0.3), value: elapsed)
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
    @State private var arrivals = 0
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.colorSchemeContrast) private var contrast

    private var symbol: String {
        switch state {
        case "waiting": "hand.raised.fill"
        case "busy": "progress.indicator"
        default: "circle"
        }
    }

    private var color: Color? {
        switch state {
        case "waiting": .orange
        case "busy": .blue
        default: nil
        }
    }

    var body: some View {
        Image(systemName: symbol)
            .symbolRenderingMode(.hierarchical)
            .foregroundStyle(color.map(AnyShapeStyle.init) ?? AnyShapeStyle(.tertiary))
            .font(.system(size: 11, weight: .semibold))
            .contentTransition(reduceMotion ? .opacity : .symbolEffect(.replace.magic(fallback: .downUp.byLayer)))
            .symbolEffect(.variableColor.iterative.dimInactiveLayers.nonReversing, options: .repeat(.continuous), isActive: state == "busy" && !reduceMotion)
            .symbolEffect(.bounce.up.byLayer, options: .nonRepeating, value: arrivals)
            .frame(width: 22, height: 22)
            .background((color ?? .clear).opacity(contrast == .increased ? 0.32 : 0.18), in: .circle)
            .overlay {
                if contrast == .increased, let color {
                    Circle().strokeBorder(color)
                }
            }
            .animation(reduceMotion ? .easeOut(duration: 0.15) : .smooth(duration: 0.3), value: state)
            .onChange(of: state) {
                if state == "waiting", !reduceMotion {
                    arrivals += 1
                }
            }
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
    let delay: Double
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
                Ring(fraction: machine.load / Double(max(machine.cores, 1)), color: machine.health, dashed: !answers, delay: delay)
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
    let color: Color
    let dashed: Bool
    let delay: Double
    @State private var drawn = false
    @Environment(\.colorSchemeContrast) private var contrast
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let width: CGFloat = contrast == .increased ? 3 : 2
        ZStack {
            Circle()
                .stroke(dashed ? AnyShapeStyle(.secondary) : AnyShapeStyle(.quaternary), style: StrokeStyle(lineWidth: width, dash: dashed ? [2.4, 2.3] : []))
            if !dashed {
                Circle()
                    .trim(from: 0, to: drawn || reduceMotion ? min(fraction, 1) : 0)
                    .stroke(color, style: StrokeStyle(lineWidth: width, lineCap: .round))
                    .rotationEffect(.degrees(-90))
            }
        }
        .frame(width: 12, height: 12)
        .onAppear {
            withAnimation(.spring(duration: 0.6, bounce: 0.15).delay(delay)) {
                drawn = true
            }
        }
    }
}

struct Press: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed && !reduceMotion ? 0.97 : 1)
            .opacity(configuration.isPressed && reduceMotion ? 0.7 : 1)
            .animation(reduceMotion ? .easeOut(duration: 0.15) : .spring(duration: 0.2, bounce: 0.3), value: configuration.isPressed)
    }
}

func span(_ seconds: TimeInterval) -> String {
    Duration.seconds(max(seconds, 60)).formatted(.units(allowed: [.days, .hours, .minutes], width: .narrow, maximumUnitCount: 1))
}

extension Machine {
    var health: Color {
        if pressure == .critical { return .red }
        if load >= 0.7 * Double(cores) || pressure == .warning || disk.pressure == .warning { return .orange }
        return .green
    }

    var device: String {
        if battery != nil { return "macbook" }
        return ssh?.isEmpty ?? true ? "desktopcomputer" : "server.rack"
    }
}
