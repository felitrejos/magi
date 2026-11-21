import AppKit
import SwiftUI

@main
struct Magi: App {
    @NSApplicationDelegateAdaptor private var app: AppDelegate
    @AppStorage("welcomed") private var welcomed = false

    var body: some Scene {
        WindowGroup("Welcome to magi", id: Welcome.id, for: String.self) { _ in
            Welcome(store: app.store)
        }
        .windowResizability(.contentSize)
        .restorationBehavior(.disabled)
        .defaultLaunchBehavior(welcomed ? .suppressed : .presented)
        Settings {
            SettingsView(store: app.store)
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let store = Store(home: .homeDirectory.appending(path: ".magi"))
    private let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
    private let icon = NSImageView()
    private let count = NSImageView()
    private let panel = Panel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
    private let menu = NSMenu()
    private let plain = NSImage.SymbolConfiguration(textStyle: .body, scale: .large)
    private var host: NSView?
    private var monitor: Any?
    private var shown: Glance?
    private var dotted: Bool?

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
        icon.setAccessibilityElement(false)
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
        panel.backgroundColor = .clear
        panel.isOpaque = false
        panel.hasShadow = false
        panel.level = .popUpMenu
        panel.hidesOnDeactivate = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.settings = { [weak self] in self?.openSettings() }
        Task { [store] in
            for await glance in Observations({ Glance(store: store) }) where glance != shown {
                show(glance)
            }
        }
        Task {
            for await _ in NotificationCenter.default.notifications(named: UserDefaults.didChangeNotification) {
                shown.map(show)
            }
        }
        if !UserDefaults.standard.bool(forKey: "welcomed") {
            NSApp.activate()
        }
    }

    private func show(_ glance: Glance) {
        let dotted = UserDefaults.standard.bool(forKey: "dot") && !glance.waiting.isEmpty
        if dotted != self.dotted {
            icon.symbolConfiguration = dotted ? plain.applying(.init(paletteColors: [.labelColor, .systemOrange])) : plain
            self.dotted = dotted
        }
        if glance.symbol != shown?.symbol {
            icon.image = NSImage(symbolName: glance.symbol, bundle: .main, variableValue: 0)
        }
        if let shown {
            let started = glance.waiting.filter { shown.waiting[$0.key] == nil }
            if !started.isEmpty, UserDefaults.standard.bool(forKey: "playsSound") {
                Chime.play()
            }
            for line in started.values.sorted() + shown.answering.subtracting(glance.answering).sorted().map({ "\($0) stopped answering" }) {
                NSAccessibility.post(element: NSApp as Any, notification: .announcementRequested, userInfo: [.announcement: line, .priority: NSAccessibilityPriorityLevel.high.rawValue])
            }
        }
        if glance.waiting.count != shown?.waiting.count {
            let digits = NSAttributedString(string: "\(glance.waiting.count)", attributes: [.font: NSFont.monospacedDigitSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)])
            count.image = NSImage(size: digits.size(), flipped: false) { _ in digits.draw(at: .zero); return true }
            count.image?.isTemplate = true
        }
        count.isHidden = !UserDefaults.standard.bool(forKey: "showsCount") || glance.waiting.isEmpty
        item.button?.setAccessibilityLabel(glance.label)
        shown = glance
    }

    @objc private func clicked(_ button: NSStatusBarButton) {
        guard let event = NSApp.currentEvent, event.type == .rightMouseUp else { return host == nil ? open() : close() }
        NSMenu.popUpContextMenu(menu, with: event, for: button)
    }

    @objc private func openSettings() {
        close()
        NSApp.activate()
        EnvironmentValues().openSettings()
    }

    private func open() {
        guard let window = item.button?.window, let screen = window.screen else { return }
        let height = min(560, screen.visibleFrame.height * 0.7)
        let host = NSHostingView(rootView: PanelView(store: store, height: height) { [weak self] in self?.close() }.padding([.horizontal, .bottom], PanelView.margin))
        host.sizingOptions = []
        panel.contentView = host
        self.host = host
        let x = min(window.frame.minX, screen.visibleFrame.maxX - PanelView.width) - PanelView.margin
        panel.setFrame(NSRect(x: x, y: window.frame.minY - height - PanelView.margin, width: PanelView.width + 2 * PanelView.margin, height: height + PanelView.margin), display: true)
        panel.alphaValue = 0
        panel.makeKeyAndOrderFront(nil)
        NSAnimationContext.animate(.easeOut(duration: 0.18)) {
            panel.animator().alphaValue = 1
        }
        monitor = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] _ in self?.close() }
    }

    private func close() {
        if #available(macOS 27, *), let session = item.expandedInterfaceSession {
            session.cancel()
        } else {
            hide(animated: true)
        }
    }

    private func hide(animated: Bool) {
        monitor.map(NSEvent.removeMonitor)
        monitor = nil
        host = nil
        guard animated else { return remove() }
        NSAnimationContext.animate(.easeIn(duration: 0.12)) {
            panel.animator().alphaValue = 0
        } completion: { [weak self] in
            if self?.host == nil {
                self?.remove()
            }
        }
    }

    private func remove() {
        panel.orderOut(nil)
        panel.contentView = nil
    }
}

@available(macOS 27, *)
extension AppDelegate: @MainActor NSStatusItemExpandedInterfaceDelegate {
    func statusItem(_ statusItem: NSStatusItem, didBegin session: NSStatusItemExpandedInterfaceSession) {
        host == nil ? open() : session.cancel()
    }

    func statusItemDidEndExpandedInterfaceSession(_ statusItem: NSStatusItem, animated: Bool) {
        hide(animated: animated)
    }
}

struct Glance: Equatable {
    let waiting: [String: String]
    let answering: Set<String>
    let hasMachines: Bool

    var symbol: String {
        waiting.isEmpty ? answering.isEmpty ? "orbit.slash" : "orbit" : "orbit.badge"
    }

    @MainActor init(store: Store) {
        waiting = Dictionary(uniqueKeysWithValues: store.rows.filter { $0.session.state == "waiting" }.map { ($0.id, "\($0.session.title) on \($0.machine) is waiting on you") })
        answering = store.answering
        hasMachines = !store.machines.isEmpty
    }

    var label: String {
        if !waiting.isEmpty { return "magi, \(waiting.count) waiting on you" }
        if !hasMachines { return "magi, not set up" }
        return answering.isEmpty ? "magi, no machine is answering" : "magi"
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
