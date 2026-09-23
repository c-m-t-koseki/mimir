import Cocoa
import SwiftUI

/// The settings the window edits, read from and written to config.toml
/// through the engine (`get_config` / `set_config`). Every change restarts
/// the engine, which is what applies it (settings load at `init`).
final class SettingsModel: ObservableObject {
    @Published var partHint = "completion"
    @Published var predictive = false
    @Published var model = ""
    @Published var lightModel = ""
    @Published var learning = true
    @Published var models: [String] = []
    @Published var error: String?

    /// Values as last read or written, by `section.key`: SwiftUI's onChange
    /// also fires for what `load` just set, which must not be written back.
    private var saved: [String: String] = [:]
    /// Pending engine restart: changes made in quick succession apply with
    /// one restart, and none lands on a process still starting up.
    private var restartWork: DispatchWorkItem?

    func load() {
        guard let data = engineClient.sendRequestSync(method: "get_config", params: [:], timeout: 2),
            let config = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            error = "設定を読めませんでした（エンジンが起動していない可能性があります）"
            return
        }
        partHint = config["part_hint"] as? String ?? partHint
        predictive = config["predictive"] as? Bool ?? predictive
        model = config["model"] as? String ?? model
        lightModel = config["light_model"] as? String ?? lightModel
        learning = config["learning"] as? Bool ?? learning
        models = config["models"] as? [String] ?? models
        saved = [
            "display.part_hint": "\(partHint)", "conversion.predictive": "\(predictive)",
            "conversion.model": model, "conversion.light_model": lightModel,
            "learning.enabled": "\(learning)",
        ]
        error = nil
    }

    func set(_ section: String, _ key: String, _ value: Any) {
        let id = "\(section).\(key)"
        guard saved[id] != "\(value)" else { return }
        let params: [String: Any] = ["section": section, "key": key, "value": value]
        guard engineClient.sendRequestSync(method: "set_config", params: params, timeout: 2) != nil
        else {
            error = "[\(section)] \(key) を保存できませんでした"
            load()
            return
        }
        saved[id] = "\(value)"
        error = nil
        restartWork?.cancel()
        let work = DispatchWorkItem { engineProcess.restart() }
        restartWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8, execute: work)
    }
}

struct SettingsView: View {
    @ObservedObject var model: SettingsModel

    var body: some View {
        Form {
            Section("入力") {
                Picker("変換中の補助表示", selection: $model.partHint) {
                    Text("補完候補").tag("completion")
                    Text("読み").tag("reading")
                    Text("表示しない").tag("none")
                }
                .onChange(of: model.partHint) { model.set("display", "part_hint", $0) }
                Toggle("予測候補を候補窓に出す", isOn: $model.predictive)
                    .onChange(of: model.predictive) { model.set("conversion", "predictive", $0) }
                Toggle("変換結果を学習する", isOn: $model.learning)
                    .onChange(of: model.learning) { model.set("learning", "enabled", $0) }
            }
            Section("変換モデル") {
                Picker("メイン", selection: $model.model) {
                    ForEach(model.models, id: \.self) { Text($0).tag($0) }
                }
                .onChange(of: model.model) { model.set("conversion", "model", $0) }
                Picker("軽量（ビーム・自動切替）", selection: $model.lightModel) {
                    ForEach(model.models, id: \.self) { Text($0).tag($0) }
                }
                .onChange(of: model.lightModel) { model.set("conversion", "light_model", $0) }
            }
            if let error = model.error {
                Text(error).foregroundStyle(Color(nsColor: Swiss.accent))
            }
            Text("変更はすぐに config.toml に保存され、エンジンを再起動して反映されます。")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
        .formStyle(.grouped)
        .frame(width: 440)
        .fixedSize()
    }
}

/// The settings window, opened from the input menu. The IME runs as a
/// background-only process, so it becomes an accessory app while the window
/// is open (to take keyboard focus) and goes back when it closes.
final class SettingsWindowController: NSObject, NSWindowDelegate {
    static let shared = SettingsWindowController()

    private let model = SettingsModel()
    private lazy var window: NSWindow = {
        let window = NSWindow(contentViewController: NSHostingController(rootView: SettingsView(model: model)))
        window.title = "Karukan 設定"
        window.styleMask = [.titled, .closable]
        window.isReleasedWhenClosed = false
        window.delegate = self
        return window
    }()

    func show() {
        model.load()
        NSApp.setActivationPolicy(.accessory)
        NSApp.activate(ignoringOtherApps: true)
        window.center()
        window.makeKeyAndOrderFront(nil)
    }

    func windowWillClose(_ notification: Notification) {
        NSApp.setActivationPolicy(.prohibited)
    }
}
