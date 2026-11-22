import AppKit
import SwiftUI
import UserNotifications

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
            SettingsView(store: app.store, permission: app.permission)
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let store = Store(home: .homeDirectory.appending(path: ".magi"))
    let permission = Permission()
    private let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
    private let icon = NSImageView()
    private let panel = Panel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
    private let menu = NSMenu()
    private let plain = NSImage.SymbolConfiguration(pointSize: 14.5, weight: .regular, scale: .large)
    private var host: NSView?
    private var monitor: Any?
    private var shown: Glance?
    private var dotted: Bool?

    func applicationWillFinishLaunching(_ notification: Notification) {
        Defaults.register()
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
        icon.setAccessibilityElement(false)
        icon.translatesAutoresizingMaskIntoConstraints = false
        button.addSubview(icon)
        NSLayoutConstraint.activate([
            icon.centerXAnchor.constraint(equalTo: button.centerXAnchor),
            icon.centerYAnchor.constraint(equalTo: button.centerYAnchor),
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
        UNUserNotificationCenter.current().delegate = self
        Task { await permission.refresh() }
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

    func applicationDidBecomeActive(_ notification: Notification) {
        Task { await permission.refresh() }
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
        let started = glance.started(after: shown)
        if !started.isEmpty, UserDefaults.standard.bool(forKey: "playsSound") {
            Chime.play()
        }
        let posted = Array(started.keys) + glance.asking(after: shown)
        if !posted.isEmpty, UserDefaults.standard.bool(forKey: "notifies") {
            notify(posted)
        }
        if shown == nil {
            withdrawStale()
        }
        withdraw(glance.stopped(after: shown))
        for line in started.values.map(\.announcement).sorted() + (shown?.answering.subtracting(glance.answering).sorted() ?? []).map({ "\($0) stopped answering" }) {
            NSAccessibility.post(element: NSApp as Any, notification: .announcementRequested, userInfo: [.announcement: line, .priority: NSAccessibilityPriorityLevel.high.rawValue])
        }
        item.length = icon.fittingSize.width
        item.button?.setAccessibilityLabel(glance.label)
        shown = glance
    }

    private func notify(_ ids: [String]) {
        Task {
            guard await permission.request() else { return }
            for id in ids {
                guard let waiting = shown?.waiting[id] else { continue }
                try? await UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: id, content: waiting.content(), trigger: nil))
            }
        }
    }

    private func withdrawStale() {
        Task {
            let delivered = await UNUserNotificationCenter.current().deliveredNotifications().map(\.request.identifier)
            guard let shown else { return }
            withdraw(shown.stale(delivered))
        }
    }

    private func withdraw(_ ids: [String]) {
        guard !ids.isEmpty else { return }
        UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: ids)
    }

    fileprivate func respond(to id: String) {
        guard let route = store.rows.first(where: { $0.id == id })?.route else { return open() }
        Opener.open(route)
        close()
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
        guard host == nil, let window = item.button?.window, let screen = window.screen else { return }
        Apps.forgetMissing()
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
        monitor = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] _ in
            guard let self, !itemTakesClick else { return }
            close()
        }
    }

    private var itemTakesClick: Bool {
        guard item.button?.window?.frame.contains(NSEvent.mouseLocation) == true else { return false }
        if #available(macOS 27, *) {
            return item.expandedInterfaceSession == nil
        }
        return true
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

extension AppDelegate: UNUserNotificationCenterDelegate {
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        [.banner, .list]
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        await respond(to: response.notification.request.identifier)
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
