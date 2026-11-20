import AppKit
import SwiftUI

@main
@MainActor
final class App: NSObject, NSApplicationDelegate {
    private let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
    private let icon = NSImageView()
    private let count = NSTextField(labelWithString: "")
    private let panel = Panel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
    private let glass = NSGlassEffectView()
    private let menu = NSMenu()
    private let store: Store
    private let scenes: NSHostingSceneRepresentation<SettingsScenes>
    private var host: Host?
    private var monitor: Any?
    private var shown: Glance?

    override init() {
        let store = Store(home: .homeDirectory.appending(path: ".magi"))
        self.store = store
        scenes = NSHostingSceneRepresentation { SettingsScenes(store: store) }
        super.init()
    }

    static func main() {
        let app = App()
        NSApplication.shared.delegate = app
        NSApplication.shared.run()
    }

    func applicationWillFinishLaunching(_ notification: Notification) {
        NSApplication.shared.addSceneRepresentation(scenes)
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let button = item.button!
        button.target = self
        button.action = #selector(clicked)
        if #available(macOS 27, *) {
            item.expandedInterfaceDelegate = self
            button.sendAction(on: .rightMouseUp)
        } else {
            button.sendAction(on: [.leftMouseUp, .rightMouseUp])
        }
        icon.symbolConfiguration = .init(textStyle: .body, scale: .large)
        icon.setAccessibilityElement(false)
        count.font = .monospacedDigitSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
        count.setAccessibilityElement(false)
        let stack = NSStackView(views: [icon, count])
        stack.spacing = 3
        stack.edgeInsets = NSEdgeInsets(top: 0, left: 4, bottom: 0, right: 4)
        stack.translatesAutoresizingMaskIntoConstraints = false
        button.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: button.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: button.trailingAnchor),
            stack.centerYAnchor.constraint(equalTo: button.centerYAnchor),
        ])
        menu.addItem(withTitle: "Settings\u{2026}", action: #selector(openSettings), keyEquivalent: ",").target = self
        menu.addItem(.separator())
        menu.addItem(withTitle: "Quit magi", action: #selector(NSApplication.terminate), keyEquivalent: "q")
        glass.cornerRadius = PanelView.radius
        glass.wantsLayer = true
        glass.layer?.cornerRadius = PanelView.radius
        glass.layer?.cornerCurve = .continuous
        glass.layer?.masksToBounds = true
        panel.contentView = glass
        panel.backgroundColor = .clear
        panel.isOpaque = false
        panel.hasShadow = true
        panel.level = .popUpMenu
        panel.hidesOnDeactivate = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.settings = { [weak self] in self?.openSettings() }
        Task { [store] in
            for await glance in Observations({ Glance(store: store) }) {
                show(glance)
            }
        }
        NotificationCenter.default.addObserver(self, selector: #selector(defaultsChanged), name: UserDefaults.didChangeNotification, object: nil)
        if !UserDefaults.standard.bool(forKey: "welcomed") {
            NSApp.activate()
            scenes.environment.openWindow(id: "welcome")
        }
    }

    private func show(_ glance: Glance) {
        let reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        let image = NSImage(symbolName: glance.symbol, bundle: .main, variableValue: 0)!
        if shown == nil || reduceMotion {
            icon.image = image
        } else if shown?.symbol != glance.symbol {
            icon.setSymbolImage(image, contentTransition: .replace.magic(fallback: .replace.downUp))
        }
        if let shown {
            let started = glance.waiting.filter { shown.waiting[$0.key] == nil }
            if !started.isEmpty, !reduceMotion {
                icon.addSymbolEffect(.bounce, options: .nonRepeating)
            }
            for line in started.values.sorted() + shown.answering.subtracting(glance.answering).sorted().map({ "\($0) stopped answering" }) {
                NSAccessibility.post(element: NSApp as Any, notification: .announcementRequested, userInfo: [.announcement: line, .priority: NSAccessibilityPriorityLevel.high.rawValue])
            }
        }
        count.stringValue = "\(glance.waiting.count)"
        count.isHidden = !UserDefaults.standard.bool(forKey: "showsCount") || glance.waiting.isEmpty
        item.button?.setAccessibilityLabel(glance.label)
        shown = glance
    }

    @objc private func defaultsChanged() {
        shown.map(show)
    }

    @objc private func clicked(_ button: NSStatusBarButton) {
        guard let event = NSApp.currentEvent, event.type == .rightMouseUp else { return panel.isVisible ? close() : open() }
        NSMenu.popUpContextMenu(menu, with: event, for: button)
    }

    @objc private func openSettings() {
        close()
        NSApp.activate()
        scenes.environment.openSettings()
    }

    private func open() {
        let host = Host(rootView: PanelView(store: store) { [weak self] in self?.close() })
        host.resized = { [weak self] in self?.fit(animate: true) }
        glass.contentView = host
        self.host = host
        fit(animate: false)
        panel.makeKeyAndOrderFront(nil)
        monitor = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] _ in self?.close() }
    }

    private func fit(animate: Bool) {
        guard let host, let window = item.button?.window, let screen = window.screen else { return }
        let height = min(host.fittingSize.height, 560, screen.visibleFrame.height * 0.7)
        let x = min(window.frame.minX, screen.visibleFrame.maxX - host.fittingSize.width)
        let frame = NSRect(x: x, y: window.frame.minY - height, width: host.fittingSize.width, height: height)
        panel.setFrame(frame, display: true, animate: animate && panel.frame != frame && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
        panel.invalidateShadow()
    }

    private func close() {
        if #available(macOS 27, *), let session = item.expandedInterfaceSession {
            session.cancel()
        } else {
            hide()
        }
    }

    private func hide() {
        monitor.map(NSEvent.removeMonitor)
        monitor = nil
        panel.orderOut(nil)
        glass.contentView = nil
        host = nil
    }
}

@available(macOS 27, *)
extension App: @MainActor NSStatusItemExpandedInterfaceDelegate {
    func statusItem(_ statusItem: NSStatusItem, didBegin session: NSStatusItemExpandedInterfaceSession) {
        panel.isVisible ? session.cancel() : open()
    }

    func statusItemDidEndExpandedInterfaceSession(_ statusItem: NSStatusItem, animated: Bool) {
        hide()
    }
}

struct Glance {
    static let glyph = "trio"

    let waiting: [String: String]
    let answering: Set<String>
    let hasMachines: Bool

    @MainActor init(store: Store) {
        waiting = Dictionary(uniqueKeysWithValues: store.rows.filter { $0.session.state == "waiting" }.map { ($0.id, "\($0.session.title) on \($0.machine) is waiting on you") })
        answering = store.answering
        hasMachines = !store.machines.isEmpty
    }

    var symbol: String {
        waiting.isEmpty ? answering.isEmpty ? "\(Glance.glyph).slash" : Glance.glyph : "\(Glance.glyph).badge"
    }

    var label: String {
        if !waiting.isEmpty { return "magi, \(waiting.count) waiting on you" }
        if !hasMachines { return "magi, not set up" }
        return answering.isEmpty ? "magi, no machine is answering" : "magi"
    }
}

struct SettingsScenes: Scene {
    let store: Store

    var body: some Scene {
        Settings {
            SettingsView()
        }
        WindowGroup("Welcome to magi", id: "welcome") {
            Welcome(store: store)
        }
        .windowResizability(.contentSize)
    }
}

final class Host: NSHostingView<PanelView> {
    var resized: () -> Void = {}

    override func invalidateIntrinsicContentSize() {
        super.invalidateIntrinsicContentSize()
        Task { [resized] in resized() }
    }
}

final class Panel: NSPanel {
    var settings: () -> Void = {}

    override var canBecomeKey: Bool { true }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command, event.charactersIgnoringModifiers == "," else { return super.performKeyEquivalent(with: event) }
        settings()
        return true
    }
}
