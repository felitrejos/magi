import AppKit
import SwiftUI

@main
@MainActor
final class App: NSObject, NSApplicationDelegate {
    private let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
    private let panel = Panel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
    private let menu = NSMenu()
    private let store = Store(home: .homeDirectory.appending(path: ".magi"))
    private var monitor: Any?

    static func main() {
        let app = App()
        NSApplication.shared.delegate = app
        NSApplication.shared.run()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let button = item.button!
        button.image = NSImage(systemSymbolName: "square.stack", accessibilityDescription: "magi")
        button.target = self
        button.action = #selector(clicked)
        if #available(macOS 27, *) {
            item.expandedInterfaceDelegate = self
            button.sendAction(on: .rightMouseUp)
        } else {
            button.sendAction(on: [.leftMouseUp, .rightMouseUp])
        }
        menu.addItem(withTitle: "Quit magi", action: #selector(NSApplication.terminate), keyEquivalent: "q")
        panel.level = .popUpMenu
        panel.hidesOnDeactivate = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
    }

    @objc private func clicked(_ button: NSStatusBarButton) {
        guard let event = NSApp.currentEvent, event.type == .rightMouseUp else { return panel.isVisible ? close() : open() }
        NSMenu.popUpContextMenu(menu, with: event, for: button)
    }

    private func open() {
        guard let window = item.button?.window, let screen = window.screen else { return }
        let view = NSHostingView(rootView: PanelView(store: store) { [weak self] in self?.close() })
        panel.contentView = view
        panel.setContentSize(view.fittingSize)
        panel.setFrameTopLeftPoint(NSPoint(x: min(window.frame.minX, screen.visibleFrame.maxX - panel.frame.width), y: window.frame.minY))
        panel.makeKeyAndOrderFront(nil)
        monitor = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] _ in self?.close() }
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
        panel.contentView = nil
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

final class Panel: NSPanel {
    override var canBecomeKey: Bool { true }
}
