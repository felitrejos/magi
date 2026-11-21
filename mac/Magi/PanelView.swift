import AppKit
import SwiftUI

struct PanelView: View {
    static let radius: CGFloat = 16
    static let width: CGFloat = 360
    static let margin: CGFloat = 24

    let store: Store
    let height: CGFloat
    let close: () -> Void
    @State private var scope: String?
    @State private var highlight: String?
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

    private var status: String {
        if let scope { return scope }
        if store.machines.isEmpty { return "magi isn't set up here" }
        let waiting = store.rows.count { $0.session.state == "waiting" }
        if waiting > 0 { return "\(waiting) waiting on you" }
        if store.answering.isEmpty { return "No machine is answering" }
        let busy = store.rows.count { $0.session.state == "busy" }
        return busy > 0 ? "\(busy) working" : "All quiet"
    }

    var body: some View {
        ScrollViewReader { proxy in
            Cap(height: height) {
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
                            SessionList(rows: rows, scope: scope, machine: scope.flatMap { store.machines[$0] }, showsMachine: scope == nil && names.count > 1, now: context.date, highlight: $highlight, order: $order, open: open)
                        }
                        if names.count > 1 {
                            strip(now: context.date)
                        }
                    }
                }
                .frame(width: PanelView.width)
                .scaleEffect(shown || reduceMotion ? 1 : 0.96, anchor: .top)
                .blur(radius: shown || reduceMotion ? 0 : 6)
                .opacity(shown ? 1 : 0)
                .containerShape(.rect(cornerRadius: PanelView.radius))
                .glassEffect(.regular, in: .rect(cornerRadius: PanelView.radius))
                .shadow(color: .black.opacity(0.18), radius: 12, y: 6)
            }
            .frame(maxHeight: .infinity, alignment: .top)
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
            .onKeyPress(.return) {
                guard let route = rows.first(where: { $0.id == highlight })?.route else { return .ignored }
                open(route)
                return .handled
            }
            .onExitCommand { scope == nil ? close() : (scope = nil) }
        }
    }

    private func header(now: Date) -> some View {
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 0) {
                Text(status)
                    .font(.title3.weight(.semibold))
                    .contentTransition(.opacity)
                    .transaction(value: status) { $0.animation = $0.animation ?? .snappy(duration: 0.35) }
                if let scope, let machine = store.machines[scope] {
                    ScopeCaption(machine: machine, answers: store.answering.contains(scope), now: now)
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
            withAnimation(reduceMotion ? nil : .smooth(duration: 0.4)) {
                scope = scope == name ? nil : name
                highlight = nil
            }
        } label: {
            Tile(name: name, machine: machine, answers: store.answering.contains(name), selected: scope == name, now: now)
        }
        .buttonStyle(Press())
    }

    private func open(_ route: Route) {
        Opener.open(route)
        close()
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

struct Cap: Layout {
    let height: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout Void) -> CGSize {
        let ideal = subviews[0].sizeThatFits(ProposedViewSize(width: proposal.width, height: nil))
        return ideal.height > height ? subviews[0].sizeThatFits(ProposedViewSize(width: proposal.width, height: height)) : ideal
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout Void) {
        subviews[0].place(at: bounds.origin, proposal: ProposedViewSize(bounds.size))
    }
}

struct SessionList: View {
    let rows: [Row]
    let scope: String?
    let machine: Machine?
    let showsMachine: Bool
    let now: Date
    @Binding var highlight: String?
    @Binding var order: [String]?
    let open: (Route) -> Void
    @State private var hovering = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ScrollView {
            VStack(spacing: 0) {
                if let scope, let machine {
                    Monitor(name: scope, machine: machine, now: now)
                        .padding(8)
                        .padding(.bottom, 6)
                        .transition(.move(edge: .top).combined(with: .opacity))
                    if !rows.isEmpty {
                        Text("Sessions")
                            .font(.footnote.weight(.semibold))
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 8)
                            .padding(.bottom, 2)
                            .accessibilityAddTraits(.isHeader)
                            .transition(.opacity)
                    }
                }
                ForEach(rows) { row in
                    SessionRow(row: row, showsMachine: showsMachine, highlighted: highlight == row.id, usage: scope != nil, now: now, open: open)
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
            .transaction(value: rows.map(\.id)) { [reduceMotion] in $0.animation = $0.animation ?? (reduceMotion ? nil : .snappy(duration: 0.35)) }
            .transaction(value: highlight) { $0.animation = $0.animation ?? .easeOut(duration: 0.15) }
        }
        .scrollIndicators(.never)
        .scrollBounceBehavior(.basedOnSize)
        .scrollEdgeEffectStyle(.hard, for: .bottom)
        .onHover { hovering = $0 }
        .task(id: hovering || highlight != nil) {
            guard !hovering, highlight == nil else { return order = order ?? rows.map(\.id) }
            guard (try? await Task.sleep(for: .seconds(1))) != nil else { return }
            order = nil
        }
    }
}

struct SessionRow: View {
    let row: Row
    let showsMachine: Bool
    let highlighted: Bool
    let usage: Bool
    let now: Date
    let open: (Route) -> Void

    private var waiting: Bool {
        row.session.state == "waiting"
    }

    private var folder: String {
        URL(filePath: row.session.folder).lastPathComponent
    }

    private var elapsed: String? {
        guard let since = row.session.since, now.timeIntervalSince(since) >= 60 else { return nil }
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
            HStack(spacing: 6) {
                if let route = row.route {
                    HostIcon(host: route.host)
                }
                if usage {
                    Usage(session: row.session, now: now)
                } else {
                    Text(elapsed ?? "now")
                        .font(.subheadline)
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                        .contentTransition(.opacity)
                        .animation(.easeOut(duration: 0.2), value: elapsed)
                }
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
        .onTapGesture {
            row.route.map(open)
        }
        .help(row.route.map { "Open in \($0.host.name)" } ?? "")
        .accessibilityElement(children: .combine)
        .accessibilityLabel(accessibility)
        .accessibilityAddTraits(row.route == nil ? [] : .isButton)
        .accessibilityActions {
            if let route = row.route {
                Button("Open in \(route.host.name)") { open(route) }
            }
        }
    }

    private var accessibility: String {
        let state = switch row.session.state {
        case "waiting": "waiting on you" + (row.session.waitingOn.map { ": \($0)" } ?? "")
        case "busy": "working"
        default: row.session.state
        }
        let detail = usage ? "CPU \(row.session.cpu)%, memory \(row.session.memory)" : elapsed.map { "for \($0)" }
        return [row.session.title, state, "\(folder) on \(row.machine)", detail].compactMap(\.self).joined(separator: ", ")
    }
}

struct HostIcon: View {
    let host: Host

    var body: some View {
        icon
            .frame(width: 16, height: 16)
            .accessibilityHidden(true)
    }

    @ViewBuilder private var icon: some View {
        if let bundle = host.bundle, let image = Apps.icon(bundle) {
            Image(nsImage: image)
                .resizable()
                .interpolation(.high)
        } else {
            Image(systemName: host.symbol)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(.secondary)
        }
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
        super.init(frame: .zero)
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
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed && !reduceMotion ? 0.97 : 1)
            .opacity(configuration.isPressed && reduceMotion ? 0.7 : 1)
            .animation(reduceMotion ? .easeOut(duration: 0.15) : .spring(duration: 0.2, bounce: 0.3), value: configuration.isPressed)
    }
}

struct Tile: View {
    let name: String
    let machine: Machine
    let answers: Bool
    let selected: Bool
    let now: Date
    @State private var hovered = false
    @Environment(\.colorSchemeContrast) private var contrast
    @Environment(\.accessibilityShowBorders) private var showBorders
    @AppStorage(Defaults.modules) private var modules = Modules.defaults

    private var caption: (text: String, health: Health)? {
        if let alert = machine.alert(modules.enabled), let word = alert.alert {
            return (word, alert.health)
        }
        return machine.measures(modules.enabled).first.map { ($0.headline, $0.health) }
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
                Image(systemName: "battery.\(battery.percent / 25 * 25)percent")
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
            if let caption {
                HStack(spacing: 5) {
                    Circle()
                        .fill(caption.health.color)
                        .frame(width: 6, height: 6)
                    Text(caption.text)
                        .font(.footnote)
                        .monospacedDigit()
                        .foregroundStyle(caption.health == .fine ? AnyShapeStyle(.secondary) : AnyShapeStyle(caption.health.color))
                        .contentTransition(.opacity)
                }
                .animation(.easeOut(duration: 0.2), value: caption.text)
            }
        } else {
            ViewThatFits {
                Text(machine.quiet(now: now))
                Text("not answering")
            }
            .font(.footnote)
            .foregroundStyle(.secondary)
        }
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
        .animation(.easeOut(duration: 0.15), value: hovered)
        .contentShape(.rect)
        .onHover { hovered = $0 }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label)
        .accessibilityHint(selected ? "Shows all sessions" : "Shows details")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private var label: String {
        [name, answers ? caption?.text : machine.quiet(now: now)].compactMap(\.self).joined(separator: ", ")
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

func span(_ seconds: TimeInterval) -> String {
    Duration.seconds(max(seconds, 60)).formatted(.units(allowed: [.days, .hours, .minutes], width: .narrow, maximumUnitCount: 1))
}

extension Machine {
    func quiet(now: Date) -> String {
        guard let heard else { return "not answering" }
        return "not answering for \(span(now.timeIntervalSince(heard)))"
    }

    var device: String {
        if battery != nil { return "macbook" }
        return ssh?.isEmpty ?? true ? "desktopcomputer" : "server.rack"
    }
}
