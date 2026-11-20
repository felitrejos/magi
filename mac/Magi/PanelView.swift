import AppKit
import SwiftUI

struct PanelView: View {
    static let radius: CGFloat = 16

    let store: Store
    let close: () -> Void
    @State private var scope: String?
    @State private var highlight: String?
    @State private var hoveredTile: String?
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

    private var status: (count: Int?, words: String) {
        if let scope { return (nil, scope) }
        if store.machines.isEmpty { return (nil, "magi isn't set up here") }
        if waiting > 0 { return (waiting, "waiting on you") }
        if store.answering.isEmpty { return (nil, "No machine is answering") }
        let busy = store.rows.count { $0.session.state == "busy" }
        return busy > 0 ? (busy, "working") : (nil, "All quiet")
    }

    var body: some View {
        ScrollViewReader { proxy in
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
            .onMoveCommand { move($0, proxy) }
            .onExitCommand { scope == nil ? close() : (scope = nil) }
            .task(id: hovering || highlight != nil) {
                guard !hovering, highlight == nil else { return order = order ?? rows.map(\.id) }
                try? await Task.sleep(for: .seconds(1))
                order = nil
            }
        }
    }

    private func header(now: Date) -> some View {
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .firstTextBaseline, spacing: 0) {
                    if let count = status.count {
                        Text("\(count) ")
                            .contentTransition(reduceMotion ? .opacity : .numericText(value: Double(count)))
                    }
                    Text(status.words)
                        .contentTransition(.opacity)
                }
                .font(.title3.weight(.semibold))
                .animation(.snappy(duration: 0.35), value: status.count)
                .animation(.snappy(duration: 0.35), value: status.words)
                .accessibilityElement(children: .combine)
                if let scope, let machine = store.machines[scope] {
                    ScopeCaption(machine: machine, answers: store.answering.contains(scope), now: now)
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
        ScrollView {
            VStack(spacing: 0) {
                if let scope, let machine = store.machines[scope] {
                    ScopeSummary(name: scope, machine: machine, now: now)
                        .padding(.bottom, 8)
                        .transition(.opacity)
                }
                ForEach(rows) { row in
                    SessionRow(row: row, showsMachine: scope == nil && names.count > 1, highlighted: highlight == row.id, usage: scope != nil, now: now)
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
            }
            .padding(.horizontal, 8)
            .padding(.bottom, 8)
            .animation(reduceMotion ? nil : .snappy(duration: 0.35), value: rows.map(\.id))
            .animation(.easeOut(duration: 0.15), value: highlight)
        }
        .scrollBounceBehavior(.basedOnSize)
        .scrollEdgeEffectStyle(.hard, for: .bottom)
        .onHover { hovering = $0 }
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
            Tile(name: name, machine: machine, answers: store.answering.contains(name), selected: scope == name, hovered: hoveredTile == name, delay: 0.08 + 0.03 * Double(order), now: now)
        }
        .buttonStyle(Press())
        .onHover { inside in
            if inside {
                hoveredTile = name
            } else if hoveredTile == name {
                hoveredTile = nil
            }
        }
    }

    private func move(_ direction: MoveCommandDirection, _ proxy: ScrollViewProxy) {
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
            proxy.scrollTo(ids[next])
        }
    }
}

struct SessionRow: View {
    let row: Row
    let showsMachine: Bool
    let highlighted: Bool
    let usage: Bool
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
            if usage {
                Usage(session: row.session, now: now)
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

    private var spins: Bool {
        state == "busy" && !reduceMotion
    }

    var body: some View {
        Image(systemName: symbol)
            .symbolRenderingMode(.hierarchical)
            .foregroundStyle(color.map(AnyShapeStyle.init) ?? AnyShapeStyle(.tertiary))
            .font(.system(size: 11, weight: .semibold))
            .contentTransition(reduceMotion ? .opacity : .symbolEffect(.replace.magic(fallback: .downUp.byLayer)))
            .symbolEffect(.bounce.up.byLayer, options: .nonRepeating, value: arrivals)
            .opacity(spins ? 0 : 1)
            .overlay {
                if spins {
                    Spinner()
                        .frame(width: 13, height: 13)
                        .transition(.opacity)
                }
            }
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

struct Spinner: NSViewRepresentable {
    func makeNSView(context: Context) -> SpinnerView {
        SpinnerView()
    }

    func updateNSView(_ view: SpinnerView, context: Context) {}
}

final class SpinnerView: NSView {
    private let spokes = CAReplicatorLayer()
    private let spoke = CALayer()

    init() {
        super.init(frame: CGRect(x: 0, y: 0, width: 13, height: 13))
        wantsLayer = true
        spokes.instanceCount = 8
        spokes.instanceTransform = CATransform3DMakeRotation(-.pi / 4, 0, 0, 1)
        spokes.instanceDelay = 0.1
        spoke.cornerRadius = 0.8
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = 1
        fade.toValue = 0.25
        fade.duration = 0.8
        fade.repeatCount = .infinity
        spoke.add(fade, forKey: "fade")
        spokes.addSublayer(spoke)
        layer?.addSublayer(spokes)
    }

    required init?(coder: NSCoder) {
        nil
    }

    override func layout() {
        super.layout()
        spokes.frame = bounds
        spoke.frame = CGRect(x: bounds.midX - 0.8, y: bounds.maxY - 4.2, width: 1.6, height: 4.2)
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        effectiveAppearance.performAsCurrentDrawingAppearance {
            spoke.backgroundColor = NSColor.systemBlue.cgColor
        }
    }
}

struct Press: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.environment(\.pressed, configuration.isPressed)
    }
}

extension EnvironmentValues {
    @Entry var pressed = false
}

struct Tile: View {
    let name: String
    let machine: Machine
    let answers: Bool
    let selected: Bool
    let hovered: Bool
    let delay: Double
    let now: Date
    @Environment(\.pressed) private var pressed
    @Environment(\.colorSchemeContrast) private var contrast
    @Environment(\.accessibilityShowBorders) private var showBorders
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var percent: Int {
        Int((machine.load / Double(max(machine.cores, 1)) * 100).rounded())
    }

    private var heading: some View {
        HStack(spacing: 4) {
            Image(systemName: machine.device)
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
            Text(name)
                .font(.subheadline.weight(.semibold))
                .truncationMode(.middle)
            if let battery = machine.battery, battery.onBattery {
                Image(systemName: "battery.\(min(battery.percent / 25 * 25, 100))percent")
                    .font(.system(size: 9))
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 2)
            Image(systemName: "chevron.right")
                .font(.system(size: 9, weight: .semibold))
                .foregroundStyle(selected || hovered ? HierarchicalShapeStyle.secondary : .tertiary)
                .rotationEffect(.degrees(selected ? -90 : 0))
        }
    }

    @ViewBuilder private var summary: some View {
        if answers {
            HStack(spacing: 6) {
                Trend(points: machine.loadPoints(now: now), color: machine.health, delay: delay)
                    .frame(minWidth: 36, maxHeight: 12)
                ViewThatFits {
                    figure("\(percent)% load")
                    figure("\(percent)%")
                }
                .layoutPriority(1)
            }
        } else {
            ViewThatFits {
                Text(quiet)
                Text("not answering")
            }
            .font(.footnote)
            .foregroundStyle(.secondary)
        }
    }

    private func figure(_ text: String) -> some View {
        Text(text)
            .font(.footnote)
            .monospacedDigit()
            .foregroundStyle(.secondary)
            .contentTransition(reduceMotion ? .opacity : .numericText(value: Double(percent)))
            .animation(.snappy(duration: 0.3), value: percent)
    }

    private var quiet: String {
        machine.quiet(now: now)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            heading
            summary
        }
        .lineLimit(1)
        .padding(.horizontal, 8)
        .frame(height: 44)
        .background { plate }
        .opacity(answers ? 1 : 0.6)
        .scaleEffect(pressed && !reduceMotion ? 0.97 : 1)
        .opacity(pressed && reduceMotion ? 0.7 : 1)
        .animation(press, value: pressed)
        .animation(.easeOut(duration: 0.15), value: hovered)
        .animation(.smooth(duration: 0.3), value: selected)
        .contentShape(.rect)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label)
        .accessibilityHint(selected ? "Shows all sessions" : "Shows details")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private var press: Animation {
        reduceMotion ? .easeOut(duration: 0.15) : .spring(duration: 0.2, bounce: 0.3)
    }

    private var label: String {
        answers ? "\(name), load \(percent)%" : "\(name), \(quiet)"
    }

    private var plate: some View {
        let shape = ConcentricRectangle(corners: .concentric(minimum: 8), isUniform: true)
        return shape
            .fill(selected || hovered ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.quaternary))
            .overlay {
                if selected || contrast == .increased || showBorders {
                    shape.stroke(.separator)
                }
            }
    }
}

struct Trend: View {
    let points: [CGPoint]
    let color: Color
    let delay: Double
    @State private var drawn = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.colorSchemeContrast) private var contrast

    var body: some View {
        ZStack {
            Trace(points: points, closed: true)
                .fill(color.opacity(0.18))
            Trace(points: points, closed: false)
                .stroke(color, style: StrokeStyle(lineWidth: contrast == .increased ? 2 : 1.25, lineCap: .round, lineJoin: .round))
        }
        .mask(alignment: .leading) {
            Rectangle().scaleEffect(x: drawn || reduceMotion ? 1 : 0, anchor: .leading)
        }
        .onAppear {
            withAnimation(.spring(duration: 0.6, bounce: 0.15).delay(delay)) {
                drawn = true
            }
        }
    }
}

struct Trace: Shape {
    let points: [CGPoint]
    let closed: Bool

    func path(in rect: CGRect) -> Path {
        Path { path in
            let placed = points.map { CGPoint(x: rect.minX + $0.x * rect.width, y: rect.maxY - $0.y * rect.height) }
            path.addLines(placed)
            if closed, let first = placed.first, let last = placed.last {
                path.addLine(to: CGPoint(x: last.x, y: rect.maxY))
                path.addLine(to: CGPoint(x: first.x, y: rect.maxY))
                path.closeSubpath()
            }
        }
    }
}

func span(_ seconds: TimeInterval) -> String {
    Duration.seconds(max(seconds, 60)).formatted(.units(allowed: [.days, .hours, .minutes], width: .narrow, maximumUnitCount: 1))
}

func trend(_ readings: [(at: TimeInterval, value: Double)], top: Double, now: Date) -> [CGPoint] {
    readings.map { CGPoint(x: 1 - (now.timeIntervalSince1970 - $0.at) / historyWindow, y: min($0.value / top, 1)) }
}

let historyWindow: TimeInterval = 60 * 60

extension Machine {
    var health: Color {
        if pressure == .critical { return .red }
        if load >= 0.7 * Double(cores) || pressure == .warning || disk.pressure == .warning { return .orange }
        return .green
    }

    func quiet(now: Date) -> String {
        guard let heard else { return "not answering" }
        return "not answering for \(span(now.timeIntervalSince(heard)))"
    }

    var device: String {
        if battery != nil { return "macbook" }
        return ssh?.isEmpty ?? true ? "desktopcomputer" : "server.rack"
    }

    var peak: Double {
        max(Double(cores), (history ?? []).map(\.load).max() ?? 0)
    }

    func loadPoints(now: Date) -> [CGPoint] {
        trend((history ?? []).map { ($0.at, $0.load) }, top: peak, now: now)
    }
}
