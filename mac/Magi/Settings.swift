import AppKit
import ServiceManagement
import SwiftUI

enum Chime {
    static let name = "Glass"
    static let volume = 0.5
    static let names = ((try? FileManager.default.contentsOfDirectory(atPath: "/System/Library/Sounds")) ?? []).map { ($0 as NSString).deletingPathExtension }.sorted()

    @MainActor static func play() {
        let sound = NSSound(named: UserDefaults.standard.string(forKey: "sound") ?? name)
        sound?.volume = Float(UserDefaults.standard.object(forKey: "volume") as? Double ?? volume)
        sound?.play()
    }
}

extension Color {
    init(light: UInt32, dark: UInt32) {
        self.init(nsColor: NSColor(name: nil) { appearance in
            let value = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light
            return NSColor(srgbRed: CGFloat(value >> 16 & 255) / 255, green: CGFloat(value >> 8 & 255) / 255, blue: CGFloat(value & 255) / 255, alpha: 1)
        })
    }
}

enum SettingsColor {
    static let content = Color(light: 0xF2F2F4, dark: 0x131315)
    static let card = Color(light: 0xFFFFFF, dark: 0x19191B)
    static let separator = Color(light: 0xE6E6E8, dark: 0x1F1F21)
    static let selection = Color(light: 0xE0E0E2, dark: 0x272729)
    static let control = Color(light: 0xE9E9EB, dark: 0x303032)
}

private let rowFont = Font.system(size: 13.5)
private let subtitleFont = Font.system(size: 11)

extension String {
    func matches(_ query: String) -> Bool {
        query.isEmpty || localizedStandardContains(query)
    }
}

extension EnvironmentValues {
    @Entry var query = ""
}

enum Pane: String, CaseIterable, Identifiable {
    case general, menuBar, monitor, sound

    var id: Self { self }

    var title: String {
        switch self {
        case .general: "General"
        case .menuBar: "Menu Bar"
        case .monitor: "Monitor"
        case .sound: "Sound"
        }
    }

    var symbol: String {
        switch self {
        case .general: "gearshape.fill"
        case .menuBar: "menubar.rectangle"
        case .monitor: "gauge.with.dots.needle.67percent"
        case .sound: "speaker.wave.2.fill"
        }
    }

    var colors: [Color] {
        switch self {
        case .general: [Color(white: 0.45), Color(white: 0.25)]
        case .menuBar: [Color(red: 0.35, green: 0.6, blue: 1), Color(red: 0.1, green: 0.35, blue: 0.9)]
        case .monitor: [Color(red: 0.3, green: 0.8, blue: 0.45), Color(red: 0.1, green: 0.55, blue: 0.25)]
        case .sound: [Color(red: 1, green: 0.4, blue: 0.5), Color(red: 0.85, green: 0.15, blue: 0.3)]
        }
    }

    func matches(_ query: String) -> Bool {
        title.matches(query) || Setting.allCases.contains { $0.pane == self && $0.title.matches(query) } || self == .monitor && Module.allCases.contains { $0.title.matches(query) }
    }
}

enum Setting: CaseIterable {
    case tutorial, login, count, dot, playsSound, sound, volume

    var title: String {
        switch self {
        case .tutorial: "Get Started with magi"
        case .login: "Open at Login"
        case .count: "Show Waiting Count"
        case .dot: "Color the Badge"
        case .playsSound: "Play a Sound"
        case .sound: "Sound"
        case .volume: "Volume"
        }
    }

    var pane: Pane {
        switch self {
        case .tutorial, .login: .general
        case .count, .dot: .menuBar
        case .playsSound, .sound, .volume: .sound
        }
    }
}

extension NSWindow {
    static func settings(store: Store, welcome: @escaping () -> Void) -> NSWindow {
        let host = NSHostingController(rootView: SettingsView(store: store, welcome: welcome))
        host.sceneBridgingOptions = [.toolbars]
        let window = NSWindow(contentViewController: host)
        window.styleMask = [.titled, .closable, .fullSizeContentView]
        window.title = "magi Settings"
        window.center()
        return window
    }
}

struct SettingsView: View {
    let store: Store
    let welcome: () -> Void
    @AppStorage("pane") private var pane = Pane.general
    @State private var back: [Pane] = []
    @State private var forward: [Pane] = []
    @State private var query = ""

    private var shown: Pane? {
        pane.matches(query) ? pane : Pane.allCases.first { $0.matches(query) }
    }

    var body: some View {
        NavigationSplitView {
            SettingsSidebar(shown: shown, query: $query, go: go)
        } detail: {
            ZStack(alignment: .topLeading) {
                ForEach(Pane.allCases) { pane in
                    if pane == shown {
                        content(pane)
                            .environment(\.query, pane.title.matches(query) ? "" : query)
                    } else {
                        content(pane)
                            .hidden()
                    }
                }
                if shown == nil {
                    ContentUnavailableView.search(text: query)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .background(SettingsColor.content)
            .toolbar {
                ToolbarItemGroup(placement: .navigation) {
                    Button("Back", systemImage: "chevron.left") {
                        forward.append(pane)
                        pane = back.removeLast()
                    }
                    .disabled(back.isEmpty)
                    Button("Forward", systemImage: "chevron.right") {
                        back.append(pane)
                        pane = forward.removeLast()
                    }
                    .disabled(forward.isEmpty)
                }
            }
            .toolbar(removing: .title)
        }
        .frame(width: 806)
    }

    private func content(_ pane: Pane) -> some View {
        VStack(alignment: .leading, spacing: 15) {
            switch pane {
            case .general: General(welcome: welcome)
            case .menuBar: MenuBar()
            case .monitor: MonitorPane(store: store)
            case .sound: Sound()
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 14)
        .padding(.bottom, 20)
    }

    private func go(_ next: Pane) {
        guard next != pane else { return }
        back.append(pane)
        forward = []
        pane = next
    }
}

struct SettingsSidebar: View {
    let shown: Pane?
    @Binding var query: String
    let go: (Pane) -> Void
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        List {
            ForEach(Pane.allCases.filter { $0.matches(query) }) { pane in
                Button {
                    go(pane)
                } label: {
                    HStack(spacing: 10) {
                        PaneTile(symbol: pane.symbol, colors: pane.colors)
                        Text(pane.title)
                            .font(rowFont)
                    }
                    .frame(maxWidth: .infinity, minHeight: 32, alignment: .leading)
                    .padding(.horizontal, 6)
                    .background(shown == pane ? SettingsColor.selection : .clear, in: .rect(cornerRadius: 8, style: .continuous))
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .listRowInsets(EdgeInsets(top: 1, leading: -6, bottom: 1, trailing: -6))
                .accessibilityAddTraits(shown == pane ? .isSelected : [])
            }
        }
        .listStyle(.sidebar)
        .background {
            if colorScheme == .dark {
                SidebarMaterial()
                    .overlay(.black.opacity(0.4))
                    .ignoresSafeArea()
            }
        }
        .searchable(text: $query, placement: .sidebar, prompt: "Search settings\u{2026}")
        .toolbar(removing: .sidebarToggle)
        .navigationSplitViewColumnWidth(232)
    }
}

struct SidebarMaterial: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .sidebar
        view.blendingMode = .behindWindow
        view.state = .followsWindowActiveState
        return view
    }

    func updateNSView(_ view: NSVisualEffectView, context: Context) {}
}

struct PaneTile: View {
    let symbol: String
    let colors: [Color]
    var size: CGFloat = 20

    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: size * 0.55, weight: .semibold))
            .symbolRenderingMode(.monochrome)
            .foregroundStyle(.white)
            .frame(width: size, height: size)
            .background(LinearGradient(colors: colors, startPoint: .top, endPoint: .bottom), in: .rect(cornerRadius: size * 0.26, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: size * 0.26, style: .continuous).strokeBorder(.white.opacity(0.15), lineWidth: 0.5))
    }
}

struct SettingCard<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        Group(subviews: content) { subviews in
            if !subviews.isEmpty {
                VStack(spacing: 0) {
                    ForEach(subviews) { subview in
                        if subview.id != subviews.first?.id {
                            SettingsColor.separator
                                .frame(height: 1)
                                .padding(.horizontal, 12)
                        }
                        subview
                    }
                }
                .background(SettingsColor.card, in: .rect(cornerRadius: 10, style: .continuous))
            }
        }
    }
}

struct SettingRow<Trailing: View>: View {
    let title: String
    var subtitle: String?
    @ViewBuilder let trailing: Trailing
    @Environment(\.query) private var query
    @Environment(\.isEnabled) private var isEnabled

    var body: some View {
        if title.matches(query) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(rowFont)
                        .foregroundStyle(isEnabled ? .primary : .secondary)
                    if let subtitle {
                        Text(subtitle)
                            .font(subtitleFont)
                            .foregroundStyle(.secondary)
                    }
                }
                Spacer(minLength: 12)
                trailing
            }
            .padding(.horizontal, 12)
            .padding(.vertical, subtitle == nil ? 11.5 : 13.5)
            .frame(minHeight: 47)
            .accessibilityElement(children: .combine)
        }
    }
}

struct SettingSwitch: View {
    @Binding var isOn: Bool

    var body: some View {
        Toggle("", isOn: $isOn)
            .labelsHidden()
            .toggleStyle(.switch)
            .controlSize(.mini)
    }
}

struct FillButton: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .medium))
            .padding(.horizontal, 12)
            .frame(height: 24)
            .background(SettingsColor.control, in: .rect(cornerRadius: 6, style: .continuous))
            .opacity(configuration.isPressed ? 0.7 : 1)
    }
}

struct General: View {
    let welcome: () -> Void
    @Environment(\.query) private var query

    var body: some View {
        if Setting.tutorial.title.matches(query) {
            SettingCard {
                HStack(spacing: 12) {
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .fill(.black)
                        .frame(width: 88, height: 49)
                        .overlay {
                            Image(.orbit)
                                .font(.title)
                                .foregroundStyle(.white)
                        }
                        .accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(Setting.tutorial.title)
                            .font(.system(size: 13, weight: .medium))
                        Text("Check that magi is set up here and how it starts.")
                            .font(subtitleFont)
                            .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 12)
                    Button("Show Steps", action: welcome)
                        .buttonStyle(FillButton())
                }
                .padding(8)
            }
        }
        SettingCard {
            LoginItem()
        }
    }
}

struct LoginItem: View {
    @State private var status = SMAppService.mainApp.status
    @State private var failure: String?

    var body: some View {
        SettingRow(title: Setting.login.title, subtitle: failure ?? (status == .requiresApproval ? "macOS needs your approval in Login Items." : nil)) {
            if status == .requiresApproval {
                Button("Open Login Items") {
                    SMAppService.openSystemSettingsLoginItems()
                }
                .buttonStyle(FillButton())
            } else {
                SettingSwitch(isOn: Binding(get: { status == .enabled }, set: { on in
                    do {
                        try on ? SMAppService.mainApp.register() : SMAppService.mainApp.unregister()
                        failure = nil
                    } catch {
                        failure = error.localizedDescription
                    }
                    status = SMAppService.mainApp.status
                }))
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            status = SMAppService.mainApp.status
        }
    }
}

struct MenuBar: View {
    @AppStorage("showsCount") private var showsCount = false
    @AppStorage("dot") private var dot = false

    var body: some View {
        SettingCard {
            SettingRow(title: Setting.count.title, subtitle: "The number of sessions waiting on you, next to the icon.") {
                SettingSwitch(isOn: $showsCount)
            }
            SettingRow(title: Setting.dot.title, subtitle: "A colored dot instead of a cutout, so it stands out.") {
                SettingSwitch(isOn: $dot)
            }
        }
    }
}

struct MonitorPane: View {
    let store: Store
    @AppStorage("monitorModules") private var modules = Modules.defaults
    @State private var picked: String?
    @State private var target: Module?
    @State private var width: CGFloat = 0
    @Environment(\.query) private var query

    private var names: [String] {
        store.machines.keys.sorted()
    }

    private var name: String? {
        picked.flatMap { store.machines[$0] == nil ? nil : $0 } ?? names.first(where: store.answering.contains) ?? names.first
    }

    private var rows: [Module] {
        modules.enabled + Module.allCases.filter { !modules.enabled.contains($0) }
    }

    var body: some View {
        if query.isEmpty, let name, let machine = store.machines[name] {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Text("Preview")
                        .font(.system(size: 13.5, weight: .semibold))
                    Spacer()
                    Menu {
                        Picker("Machine", selection: Binding(get: { name }, set: { picked = $0 })) {
                            ForEach(names, id: \.self) { name in
                                Text(name).tag(name)
                            }
                        }
                        .pickerStyle(.inline)
                    } label: {
                        HStack(spacing: 4) {
                            Image(systemName: machine.device)
                                .font(.system(size: 10))
                            Text(name)
                                .font(.system(size: 13, weight: .medium))
                            Image(systemName: "chevron.up.chevron.down")
                                .font(.system(size: 9, weight: .semibold))
                        }
                        .padding(.horizontal, 10)
                        .frame(height: 24)
                        .background(SettingsColor.control, in: .rect(cornerRadius: 6, style: .continuous))
                    }
                    .menuStyle(.button)
                    .buttonStyle(.plain)
                    .menuIndicator(.hidden)
                    .fixedSize()
                }
                MonitorGrid(name: name, measures: Array(machine.measures(modules.enabled).filter { $0.module != .network }.prefix(4)), now: .now)
                    .frame(width: 328)
                    .containerShape(.rect)
                    .frame(maxWidth: .infinity)
            }
            .padding(12)
            .background(SettingsColor.card, in: .rect(cornerRadius: 10, style: .continuous))
        }
        let matching = rows.filter { $0.title.matches(query) }
        if !matching.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Text("Modules")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .padding(.leading, 12)
                SettingCard {
                    ForEach(matching) { module in
                        row(module)
                    }
                }
                .onGeometryChange(for: CGFloat.self, of: \.size.width) { width = $0 }
                .animation(.snappy(duration: 0.3), value: modules)
                if query.isEmpty {
                    Text("Drag to reorder. Every machine shows the same modules; one without the data skips it.")
                        .font(subtitleFont)
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 12)
                }
            }
        }
        if query.isEmpty {
            HStack {
                Spacer()
                Button("Restore Defaults") {
                    modules = .defaults
                }
                .buttonStyle(FillButton())
            }
        }
    }

    @ViewBuilder private func row(_ module: Module) -> some View {
        let on = modules.enabled.contains(module)
        let row = ModuleRow(module: module, on: Binding(get: { on }, set: { enable(module, $0) }), draggable: on)
            .background(target == module ? SettingsColor.selection : .clear, in: .rect(cornerRadius: 10, style: .continuous))
        if on {
            row
                .draggable(module.rawValue) {
                    ModuleRow(module: module, on: .constant(true), draggable: true)
                        .frame(width: width)
                        .background(SettingsColor.card, in: .rect(cornerRadius: 10, style: .continuous))
                        .shadow(color: .black.opacity(0.18), radius: 8, y: 3)
                }
                .dropDestination(for: String.self) { items, _ in
                    target = nil
                    guard let moved = items.first.flatMap(Module.init(rawValue:)), let from = modules.enabled.firstIndex(of: moved), let to = modules.enabled.firstIndex(of: module) else { return }
                    modules.enabled.move(fromOffsets: [from], toOffset: to > from ? to + 1 : to)
                }
                .onDropSessionUpdated { session in
                    switch session.phase {
                    case .entering, .active: target = module
                    default: target = target == module ? nil : target
                    }
                }
        } else {
            row
        }
    }

    private func enable(_ module: Module, _ on: Bool) {
        if on {
            modules.enabled.append(module)
        } else {
            modules.enabled.removeAll { $0 == module }
        }
    }
}

struct ModuleRow: View {
    let module: Module
    @Binding var on: Bool
    let draggable: Bool

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "line.3.horizontal")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.tertiary)
                .opacity(draggable ? 1 : 0)
                .accessibilityHidden(true)
            PaneTile(symbol: module.symbol, colors: module.colors)
            VStack(alignment: .leading, spacing: 2) {
                Text(module.title)
                    .font(rowFont)
                Text(module.subtitle)
                    .font(subtitleFont)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 12)
            SettingSwitch(isOn: $on)
        }
        .padding(.horizontal, 12)
        .frame(height: 47)
        .contentShape(.rect)
        .accessibilityElement(children: .combine)
    }
}

struct Sound: View {
    @AppStorage("playsSound") private var playsSound = false
    @AppStorage("sound") private var sound = Chime.name
    @AppStorage("volume") private var volume = Chime.volume

    var body: some View {
        SettingCard {
            SettingRow(title: Setting.playsSound.title, subtitle: "Once, each time a session starts waiting on you.") {
                SettingSwitch(isOn: $playsSound)
            }
            SettingRow(title: Setting.sound.title) {
                Menu {
                    Picker(Setting.sound.title, selection: $sound) {
                        ForEach(Chime.names, id: \.self) { name in
                            Text(name).tag(name)
                        }
                    }
                    .pickerStyle(.inline)
                } label: {
                    HStack(spacing: 12) {
                        Text(sound)
                            .font(rowFont)
                        Image(systemName: "chevron.down")
                            .font(.system(size: 9, weight: .semibold))
                            .foregroundStyle(.secondary)
                    }
                    .padding(.trailing, 6)
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .menuIndicator(.hidden)
                .fixedSize()
                .onChange(of: sound) {
                    Chime.play()
                }
            }
            .disabled(!playsSound)
            SettingRow(title: Setting.volume.title) {
                HStack(spacing: 10) {
                    Slider(value: $volume, in: 0...1) { editing in
                        if !editing {
                            Chime.play()
                        }
                    }
                    .frame(width: 150)
                    Text(volume, format: .percent.precision(.fractionLength(0)))
                        .font(rowFont)
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                        .frame(minWidth: 40, alignment: .trailing)
                }
            }
            .disabled(!playsSound)
        }
    }
}

struct Welcome: View {
    static let id = "welcome"

    let store: Store
    @State private var step = 0
    @AppStorage("welcomed") private var welcomed = false
    @Environment(\.dismiss) private var dismiss

    private var status: String {
        if store.machines.isEmpty { return "Ask Claude to \u{201C}set up magi with my homelab\u{201D}." }
        if store.answering.isEmpty { return "No machine is answering. Ask Claude to \u{201C}check magi on this Mac\u{201D}." }
        return "magi is set up here."
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 12) {
                PaneTile(symbol: step == 0 ? "checkmark.seal.fill" : "power", colors: Pane.general.colors, size: 36)
                VStack(alignment: .leading, spacing: 2) {
                    Text(step == 0 ? "Is magi set up here?" : "Open magi at login?")
                        .font(.title3.weight(.semibold))
                    Text("You can skip it and run it again from Settings.")
                        .font(subtitleFont)
                        .foregroundStyle(.secondary)
                }
            }
            SettingCard {
                if step == 0 {
                    SettingRow(title: "Machines answering", subtitle: status) {
                        if !store.machines.isEmpty {
                            Text("\(store.answering.count) of \(store.machines.count)")
                                .font(rowFont)
                                .monospacedDigit()
                        }
                    }
                } else {
                    LoginItem()
                }
            }
            HStack {
                Text("Step \(step + 1) of 2")
                    .font(subtitleFont)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
                Spacer()
                Button(step == 1 ? "Done" : store.answering.isEmpty ? "Skip" : "Continue") {
                    if step == 0 {
                        step = 1
                    } else {
                        dismiss()
                    }
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 420)
        .fixedSize(horizontal: false, vertical: true)
        .windowResizeAnchor(.top)
        .containerBackground(SettingsColor.content, for: .window)
        .onDisappear {
            welcomed = true
        }
    }
}
