import AppKit
import ServiceManagement
import SwiftUI

enum Glyph: String, CaseIterable {
    case trio, orbit, bridge

    var resource: ImageResource {
        switch self {
        case .trio: .trio
        case .orbit: .orbit
        case .bridge: .bridge
        }
    }
}

enum Chime {
    static let name = "Glass"
    static let volume = 0.5
    static let names = ((try? FileManager.default.contentsOfDirectory(atPath: "/System/Library/Sounds")) ?? []).map { ($0 as NSString).deletingPathExtension }.sorted()

    @MainActor static func play() {
        let sound = NSSound(named: UserDefaults.standard.string(forKey: "chime") ?? name)
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
    static let pill = Color(light: 0xDCDCDE, dark: 0x444446)
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
    case general, menuBar, sound

    var id: Self { self }

    var title: String {
        switch self {
        case .general: "General"
        case .menuBar: "Menu Bar"
        case .sound: "Sound"
        }
    }

    var symbol: String {
        switch self {
        case .general: "gearshape.fill"
        case .menuBar: "menubar.rectangle"
        case .sound: "speaker.wave.2.fill"
        }
    }

    var colors: [Color] {
        switch self {
        case .general: [Color(white: 0.45), Color(white: 0.25)]
        case .menuBar: [Color(red: 0.35, green: 0.6, blue: 1), Color(red: 0.1, green: 0.35, blue: 0.9)]
        case .sound: [Color(red: 1, green: 0.4, blue: 0.5), Color(red: 0.85, green: 0.15, blue: 0.3)]
        }
    }

    func matches(_ query: String) -> Bool {
        title.matches(query) || Setting.allCases.contains { $0.pane == self && $0.title.matches(query) }
    }
}

enum Setting: CaseIterable {
    case tutorial, login, icon, count, dot, chimes, chime, volume

    var title: String {
        switch self {
        case .tutorial: "Get Started with magi"
        case .login: "Open at Login"
        case .icon: "Icon"
        case .count: "Show Waiting Count"
        case .dot: "Colour the Badge"
        case .chimes: "Play a Sound"
        case .chime: "Sound"
        case .volume: "Volume"
        }
    }

    var pane: Pane {
        switch self {
        case .tutorial, .login: .general
        case .icon, .count, .dot: .menuBar
        case .chimes, .chime, .volume: .sound
        }
    }
}

extension NSWindow {
    static func settings(welcome: @escaping () -> Void) -> NSWindow {
        let host = NSHostingController(rootView: SettingsView(welcome: welcome))
        host.sceneBridgingOptions = [.toolbars]
        let window = NSWindow(contentViewController: host)
        window.styleMask.insert(.fullSizeContentView)
        window.title = "magi Settings"
        window.setContentSize(NSSize(width: 806, height: 792))
        window.center()
        return window
    }
}

struct SettingsView: View {
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
            Group {
                if let shown {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 15) {
                            switch shown {
                            case .general: General(welcome: welcome)
                            case .menuBar: MenuBar()
                            case .sound: Sound()
                            }
                        }
                        .padding(.horizontal, 16)
                        .padding(.top, 14)
                        .padding(.bottom, 20)
                    }
                    .environment(\.query, shown.title.matches(query) ? "" : query)
                    .scrollEdgeEffectHidden(true, for: .top)
                    .mask {
                        VStack(spacing: 0) {
                            LinearGradient(colors: [.clear, .black], startPoint: .top, endPoint: .bottom)
                                .frame(height: 56)
                            Color.black
                        }
                        .ignoresSafeArea()
                    }
                } else {
                    ContentUnavailableView.search(text: query)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
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
        .frame(minWidth: 700, minHeight: 500)
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

extension SettingRow {
    init(_ item: Setting, subtitle: String? = nil, @ViewBuilder trailing: () -> Trailing) {
        self.init(title: item.title, subtitle: subtitle, trailing: trailing)
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
    @AppStorage("glyph") private var glyph = Glyph.orbit
    @Environment(\.query) private var query

    var body: some View {
        if Setting.tutorial.title.matches(query) {
            SettingCard {
                HStack(spacing: 12) {
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .fill(.black)
                        .frame(width: 88, height: 49)
                        .overlay {
                            Image(glyph.resource)
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

    var body: some View {
        SettingRow(.login, subtitle: status == .requiresApproval ? "macOS needs your approval in Login Items." : nil) {
            if status == .requiresApproval {
                Button("Open Login Items") {
                    SMAppService.openSystemSettingsLoginItems()
                }
                .buttonStyle(FillButton())
            } else {
                SettingSwitch(isOn: Binding(get: { status == .enabled }, set: { on in
                    try? on ? SMAppService.mainApp.register() : SMAppService.mainApp.unregister()
                    status = SMAppService.mainApp.status
                }))
            }
        }
    }
}

struct MenuBar: View {
    @AppStorage("glyph") private var glyph = Glyph.orbit
    @AppStorage("showsCount") private var showsCount = false
    @AppStorage("dot") private var dot = false
    @Environment(\.query) private var query

    var body: some View {
        if Setting.icon.title.matches(query) {
            SettingCard {
                VStack(alignment: .leading, spacing: 12) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(Setting.icon.title)
                            .font(rowFont)
                        Text("How magi looks in the menu bar.")
                            .font(subtitleFont)
                            .foregroundStyle(.secondary)
                    }
                    HStack(spacing: 0) {
                        ForEach(Glyph.allCases, id: \.self) { candidate in
                            if candidate != Glyph.allCases.first {
                                Spacer(minLength: 16)
                            }
                            Button {
                                glyph = candidate
                            } label: {
                                VStack(spacing: 6) {
                                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                                        .fill(LinearGradient(colors: [.indigo, .blue], startPoint: .topLeading, endPoint: .bottomTrailing))
                                        .frame(width: 147, height: 87)
                                        .overlay {
                                            Image(candidate.resource)
                                                .font(.system(size: 30))
                                                .foregroundStyle(.white)
                                        }
                                        .accessibilityHidden(true)
                                    Text(candidate.rawValue.capitalized)
                                        .font(.system(size: 12, weight: .medium))
                                        .padding(.horizontal, 10)
                                        .frame(height: 22)
                                        .background(glyph == candidate ? SettingsColor.pill : .clear, in: .capsule)
                                }
                                .contentShape(.rect)
                            }
                            .buttonStyle(.plain)
                            .accessibilityAddTraits(glyph == candidate ? .isSelected : [])
                        }
                    }
                }
                .padding(12)
            }
        }
        SettingCard {
            SettingRow(.count, subtitle: "The number of sessions waiting on you, next to the icon.") {
                SettingSwitch(isOn: $showsCount)
            }
            SettingRow(.dot, subtitle: "A coloured dot instead of a cutout, so it stands out.") {
                SettingSwitch(isOn: $dot)
            }
        }
    }
}

struct Sound: View {
    @AppStorage("chimes") private var chimes = false
    @AppStorage("chime") private var chime = Chime.name
    @AppStorage("volume") private var volume = Chime.volume

    var body: some View {
        SettingCard {
            SettingRow(.chimes, subtitle: "Once, each time a session starts waiting on you.") {
                SettingSwitch(isOn: $chimes)
            }
            SettingRow(.chime) {
                Menu {
                    Picker(Setting.chime.title, selection: $chime) {
                        ForEach(Chime.names, id: \.self) { name in
                            Text(name).tag(name)
                        }
                    }
                    .pickerStyle(.inline)
                } label: {
                    HStack(spacing: 12) {
                        Text(chime)
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
                .onChange(of: chime) {
                    Chime.play()
                }
            }
            .disabled(!chimes)
            SettingRow(.volume) {
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
            .disabled(!chimes)
        }
    }
}

struct Welcome: View {
    let store: Store
    @State private var step = 0
    @AppStorage("welcomed") private var welcomed = false
    @Environment(\.dismissWindow) private var dismissWindow

    var body: some View {
        VStack(alignment: .leading, spacing: 15) {
            HStack(spacing: 12) {
                PaneTile(symbol: step == 0 ? "checkmark.seal.fill" : "power", colors: Pane.general.colors, size: 36)
                VStack(alignment: .leading, spacing: 2) {
                    Text(step == 0 ? "Is magi set up here?" : "Open magi at login?")
                        .font(.title3.weight(.semibold))
                    Text("Step \(step + 1) of 2. You can skip it and run it again from Settings.")
                        .font(subtitleFont)
                        .foregroundStyle(.secondary)
                }
            }
            SettingCard {
                if step == 0 {
                    SettingRow(title: "Machines Answering", subtitle: store.answering.isEmpty ? "Ask Claude to \u{201C}set up magi with my homelab\u{201D}, and this step checks itself." : "magi is set up here.") {
                        Text("\(store.answering.count) of \(store.machines.count)")
                            .font(rowFont)
                            .monospacedDigit()
                    }
                } else {
                    LoginItem()
                }
            }
            Spacer(minLength: 0)
            HStack {
                Spacer()
                Button(step == 1 ? "Done" : store.answering.isEmpty ? "Skip" : "Continue") {
                    if step == 0 {
                        step = 1
                    } else {
                        welcomed = true
                        dismissWindow(id: "welcome")
                    }
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 480, height: 320)
        .containerBackground(SettingsColor.content, for: .window)
    }
}
