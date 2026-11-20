import ServiceManagement
import SwiftUI

struct SettingsView: View {
    var body: some View {
        TabView {
            Tab("General", systemImage: "gearshape") {
                General()
            }
            Tab("About", systemImage: "info.circle") {
                About()
            }
        }
        .frame(width: 440)
    }
}

struct General: View {
    @AppStorage("showsCount") private var showsCount = false
    @AppStorage("chimes") private var chimes = false
    @AppStorage("volume") private var volume = 0.5
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Form {
            LoginItem()
            Toggle("Show the waiting count in the menu bar", isOn: $showsCount)
            Toggle("Play a sound when a session starts waiting", isOn: $chimes)
            Slider(value: $volume, in: 0...1) {
                Text("Volume")
            }
            .disabled(!chimes)
            LabeledContent("First run") {
                Button("Show Again") {
                    NSApp.activate()
                    openWindow(id: "welcome")
                }
            }
        }
        .formStyle(.grouped)
        .fixedSize(horizontal: false, vertical: true)
    }
}

struct LoginItem: View {
    @State private var status = SMAppService.mainApp.status

    var body: some View {
        Toggle("Open magi at login", isOn: Binding(get: { status == .enabled }, set: { on in
            try? on ? SMAppService.mainApp.register() : SMAppService.mainApp.unregister()
            status = SMAppService.mainApp.status
        }))
        if status == .requiresApproval {
            LabeledContent("macOS needs your approval") {
                Button("Open Login Items") {
                    SMAppService.openSystemSettingsLoginItems()
                }
            }
        }
    }
}

struct About: View {
    var body: some View {
        VStack(spacing: 6) {
            Image(nsImage: NSApp.applicationIconImage)
                .resizable()
                .frame(width: 64, height: 64)
            Text("magi")
                .font(.title2.weight(.semibold))
            Text("Version \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "")")
                .foregroundStyle(.secondary)
            Text("Updates come through magi.")
                .foregroundStyle(.secondary)
            Text("Claude is a trademark of Anthropic. magi is not affiliated with Anthropic.")
                .font(.footnote)
                .foregroundStyle(.tertiary)
                .multilineTextAlignment(.center)
                .padding(.top, 12)
        }
        .padding(24)
        .frame(maxWidth: .infinity)
    }
}

struct Welcome: View {
    let store: Store
    @State private var step = 0
    @AppStorage("welcomed") private var welcomed = false
    @Environment(\.dismissWindow) private var dismissWindow

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Step \(step + 1) of 2")
                .font(.subheadline)
                .foregroundStyle(.secondary)
            if step == 0 {
                Text("Is magi set up here?")
                    .font(.title2.weight(.semibold))
                if store.answering.isEmpty {
                    Text("magi hasn't written anything here yet. Ask Claude to \u{201C}set up magi with my homelab\u{201D}, and this step checks itself.")
                } else {
                    Label("magi is set up. \(store.answering.count) of \(store.machines.count) machines answer.", systemImage: "checkmark.circle.fill")
                }
            } else {
                Text("Open magi at login?")
                    .font(.title2.weight(.semibold))
                Text("magi stays in the menu bar and wakes only when a machine reports.")
                Form {
                    LoginItem()
                }
                .formStyle(.grouped)
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
        .padding(24)
        .frame(width: 420, height: 280, alignment: .topLeading)
    }
}
