import AppKit
import ServiceManagement
import SwiftUI
import UserNotifications

enum Chime {
    static let name = "Glass"
    static let volume = 0.5
    static let names = ((try? FileManager.default.contentsOfDirectory(atPath: "/System/Library/Sounds")) ?? []).map { ($0 as NSString).deletingPathExtension }.sorted()

    @MainActor static func play() {
        let sound = UserDefaults.standard.string(forKey: "sound").flatMap { NSSound(named: $0) }
        sound?.volume = Float(UserDefaults.standard.double(forKey: "volume"))
        sound?.play()
    }
}

enum Defaults {
    static let modules = "monitorModules"

    static func register() {
        UserDefaults.standard.register(defaults: ["notifies": true, "sound": Chime.name, "volume": Chime.volume, modules: Modules.defaults.rawValue])
    }
}

@MainActor
@Observable
final class Permission {
    private(set) var status = UNAuthorizationStatus.notDetermined

    @discardableResult
    func request() async -> Bool {
        let granted = (try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert])) == true
        await refresh()
        return granted
    }

    func refresh() async {
        status = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
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

enum Pane: String, CaseIterable, Identifiable {
    case general, notifications, monitor

    var id: Self { self }

    var title: String {
        switch self {
        case .general: "General"
        case .notifications: "Notifications"
        case .monitor: "Monitor"
        }
    }

    var symbol: String {
        switch self {
        case .general: "gearshape.fill"
        case .notifications: "bell.badge.fill"
        case .monitor: "gauge.with.dots.needle.67percent"
        }
    }
}

struct SettingsView: View {
    let store: Store
    let permission: Permission
    @AppStorage("pane") private var pane = Pane.general

    var body: some View {
        TabView(selection: $pane) {
            ForEach(Pane.allCases) { pane in
                Tab(pane.title, systemImage: pane.symbol, value: pane) {
                    VStack(alignment: .leading, spacing: 15) {
                        switch pane {
                        case .general: General()
                        case .notifications:
                            SettingSection(title: "Notifications") { Notify(permission: permission) }
                            SettingSection(title: "Sound") { Sound() }
                        case .monitor: MonitorPane(store: store)
                        }
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 14)
                    .padding(.bottom, 20)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(width: 574)
                }
            }
        }
        .containerBackground(SettingsColor.content, for: .window)
    }
}

struct SymbolTile: View {
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

struct SettingSection<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(.secondary)
                .padding(.leading, 12)
            content
        }
    }
}

struct SettingRow<Trailing: View>: View {
    let title: String
    var subtitle: String?
    @ViewBuilder let trailing: Trailing
    @Environment(\.isEnabled) private var isEnabled

    var body: some View {
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
    @Environment(\.openWindow) private var openWindow

    var body: some View {
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
                    Text("Get Started with magi")
                        .font(.system(size: 13, weight: .medium))
                    Text("Check that magi is set up here and how it starts.")
                        .font(subtitleFont)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 12)
                Button("Show Steps") {
                    openWindow(id: Welcome.id, value: Welcome.id)
                }
                .buttonStyle(FillButton())
            }
            .padding(8)
        }
        SettingCard {
            LoginItem()
        }
        SettingSection(title: "Menu Bar") {
            MenuBar()
        }
    }
}

struct LoginItem: View {
    @State private var status = SMAppService.mainApp.status
    @State private var failure: String?

    var body: some View {
        SettingRow(title: "Open at Login", subtitle: failure ?? (status == .requiresApproval ? "macOS needs your approval in Login Items." : nil)) {
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
    @AppStorage("dot") private var dot = false

    var body: some View {
        SettingCard {
            SettingRow(title: "Color the Badge", subtitle: "A colored dot instead of a cutout, so it stands out.") {
                SettingSwitch(isOn: $dot)
            }
        }
    }
}

struct MonitorPane: View {
    let store: Store
    @AppStorage(Defaults.modules) private var modules = Modules.defaults
    @State private var picked: String?
    @State private var target: Module?
    @State private var width: CGFloat = 0

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
        if let name, let machine = store.machines[name] {
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
                Monitor(name: name, machine: machine, now: .now, open: nil)
                    .frame(width: 328)
                    .containerShape(.rect)
                    .frame(maxWidth: .infinity)
            }
            .padding(12)
            .background(SettingsColor.card, in: .rect(cornerRadius: 10, style: .continuous))
        }
        SettingSection(title: "Modules") {
            SettingCard {
                ForEach(rows) { module in
                    row(module)
                }
            }
            .onGeometryChange(for: CGFloat.self, of: \.size.width) { width = $0 }
            .animation(.snappy(duration: 0.3), value: modules)
            Text("Drag to reorder. Every machine shows the same modules; one without the data skips it.")
                .font(subtitleFont)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 12)
        }
        HStack {
            Spacer()
            Button("Restore Defaults") {
                modules = .defaults
            }
            .buttonStyle(FillButton())
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
            SymbolTile(symbol: module.symbol, colors: module.colors)
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

struct Notify: View {
    let permission: Permission
    @AppStorage("notifies") private var notifies = true

    var body: some View {
        SettingCard {
            SettingRow(title: "Notify When a Session Waits", subtitle: permission.status == .denied ? "Notifications for magi are off in System Settings." : "One notification per wait, with the question. Click it to open the session.") {
                if permission.status == .denied {
                    Button("Open Notification Settings") {
                        NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=\(Bundle.main.bundleIdentifier!)")!)
                    }
                    .buttonStyle(FillButton())
                } else {
                    SettingSwitch(isOn: $notifies)
                }
            }
        }
        .onChange(of: notifies) {
            if notifies {
                Task { await permission.request() }
            }
        }
    }
}

struct Sound: View {
    @AppStorage("playsSound") private var playsSound = false
    @AppStorage("sound") private var sound = Chime.name
    @AppStorage("volume") private var volume = Chime.volume

    var body: some View {
        SettingCard {
            SettingRow(title: "Play a Sound", subtitle: "Once, each time a session starts waiting on you.") {
                SettingSwitch(isOn: $playsSound)
            }
            SettingRow(title: "Sound") {
                Menu {
                    Picker("Sound", selection: $sound) {
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
            SettingRow(title: "Volume") {
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
                SymbolTile(symbol: step == 0 ? "checkmark.seal.fill" : "power", colors: [Color(white: 0.45), Color(white: 0.25)], size: 36)
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
