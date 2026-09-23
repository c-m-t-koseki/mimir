import Cocoa
import SwiftUI

// MARK: - Engine-backed settings

/// The settings the window edits, read from and written to config.toml
/// through the engine (`get_config` / `set_config`). Changes restart the
/// engine, which is what applies them (settings load at `init`).
final class SettingsModel: ObservableObject {
    @Published var partHint = "completion"
    @Published var predictive = false
    @Published var skkStyle = false
    @Published var model = ""
    @Published var lightModel = ""
    @Published var learning = true
    @Published var date = true
    @Published var models: [String] = []
    /// Saved model combinations: name → (main, light).
    @Published var profiles: [String: (model: String, light: String)] = [:]
    @Published var error: String?

    /// Values as last read or written, by `section.key`: SwiftUI's onChange
    /// also fires for what `load` just set, which must not be written back.
    private var saved: [String: String] = [:]

    func load() {
        guard let data = engineClient.sendRequestSync(method: "get_config", params: [:], timeout: 2),
            let config = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            error = "設定を読めませんでした（エンジンが起動していない可能性があります）"
            return
        }
        partHint = config["part_hint"] as? String ?? partHint
        predictive = config["predictive"] as? Bool ?? predictive
        skkStyle = config["skk_style"] as? Bool ?? skkStyle
        model = config["model"] as? String ?? model
        lightModel = config["light_model"] as? String ?? lightModel
        learning = config["learning"] as? Bool ?? learning
        date = config["date"] as? Bool ?? date
        models = config["models"] as? [String] ?? models
        let rawProfiles = config["profiles"] as? [String: [String: String]] ?? [:]
        profiles = rawProfiles.mapValues { ($0["model"] ?? "", $0["light_model"] ?? "") }
        saved = [
            "display.part_hint": partHint, "conversion.predictive": "\(predictive)",
            "conversion.skk_style": "\(skkStyle)",
            "conversion.model": model, "conversion.light_model": lightModel,
            "learning.enabled": "\(learning)", "date.enabled": "\(date)",
        ]
        error = nil
    }

    /// Write `[section] key = value` (`nil` removes it). Returns success.
    @discardableResult
    func set(_ section: String, _ key: String, _ value: Any?) -> Bool {
        let id = "\(section).\(key)"
        let text = value.map { "\($0)" }
        guard saved[id] != text else { return true }
        let params: [String: Any] = ["section": section, "key": key, "value": value ?? NSNull()]
        guard engineClient.sendRequestSync(method: "set_config", params: params, timeout: 2) != nil
        else {
            error = "[\(section)] \(key) を保存できませんでした"
            load()
            return false
        }
        saved[id] = text
        error = nil
        EngineRestart.schedule()
        return true
    }

    /// The saved profile matching the current models, if any.
    var currentProfile: String? {
        profiles.first { $0.value.model == model && $0.value.light == lightModel }?.key
    }

    func applyProfile(_ name: String) {
        guard let profile = profiles[name] else { return }
        model = profile.model
        lightModel = profile.light
    }

    func saveProfile(_ name: String) {
        let section = "profiles.\(name)"
        guard set(section, "model", model), set(section, "light_model", lightModel) else { return }
        profiles[name] = (model, lightModel)
    }

    func deleteProfile(_ name: String) {
        let section = "profiles.\(name)"
        guard set(section, "model", nil), set(section, "light_model", nil) else { return }
        profiles[name] = nil
    }
}

/// Engine restarts coalesce: changes made in quick succession apply with one
/// restart, and none lands on a process still starting up.
enum EngineRestart {
    private static var work: DispatchWorkItem?

    static func schedule() {
        work?.cancel()
        let next = DispatchWorkItem { engineProcess.restart() }
        work = next
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8, execute: next)
    }
}

// MARK: - Model profiling

struct ModelScore: Identifiable {
    let id: String
    let accuracy: Double?
    let avgMs: Double?
    let loadMs: Double?
    let error: String?
}

/// Runs `mimir-imserver --profile-models` (a separate process, so typing
/// never waits on it) and collects one score per model.
final class ModelProfiler: ObservableObject {
    @Published var scores: [ModelScore] = []
    @Published var running = false
    @Published var samples = ""

    func run() {
        guard !running else { return }
        running = true
        scores = []
        let server = Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/mimir-imserver")
        DispatchQueue.global(qos: .userInitiated).async {
            let process = Process()
            process.executableURL = server
            process.arguments = ["--profile-models"]
            let out = Pipe()
            process.standardOutput = out
            process.standardError = FileHandle.nullDevice
            try? process.run()
            let data = out.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            var scores: [ModelScore] = []
            var samples = ""
            for line in String(decoding: data, as: UTF8.self).split(separator: "\n") {
                guard
                    let json = try? JSONSerialization.jsonObject(with: Data(line.utf8))
                        as? [String: Any]
                else { continue }
                samples = json["samples"] as? String ?? samples
                scores.append(
                    ModelScore(
                        id: json["model"] as? String ?? "?",
                        accuracy: json["accuracy"] as? Double,
                        avgMs: json["avg_ms"] as? Double,
                        loadMs: (json["load_ms"] as? NSNumber)?.doubleValue,
                        error: json["error"] as? String))
            }
            DispatchQueue.main.async {
                self.scores = scores
                self.samples =
                    samples == "learning" ? "あなたの最近の確定（最大50件）" : "内蔵の例文（20件）"
                self.running = false
            }
        }
    }
}

// MARK: - User dictionary (Mozc TSV files in user_dicts/)

struct DictEntry: Identifiable, Hashable {
    var id: String { "\(file):\(line)" }
    let file: String
    let line: Int
    let reading: String
    let word: String
}

/// Edits the Mozc TSV files in `user_dicts/` directly (the engine reads them
/// at start, so changes restart it). Binary dictionaries are listed by name
/// only — they are built from a TSV, not edited.
final class UserDictionary: ObservableObject {
    static let dir = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/Application Support/com.mimir.mimir-im/user_dicts")
    static let registered = "registered.tsv"

    @Published var entries: [DictEntry] = []
    @Published var binaries: [String] = []

    func load() {
        let files = (try? FileManager.default.contentsOfDirectory(atPath: Self.dir.path)) ?? []
        binaries = files.filter { $0.hasSuffix(".bin") }.sorted()
        entries = files.filter { $0.hasSuffix(".tsv") }.sorted().flatMap { file in
            lines(of: file).enumerated().compactMap { index, line -> DictEntry? in
                guard !line.hasPrefix("#") else { return nil }
                let cols = line.split(separator: "\t", omittingEmptySubsequences: false)
                guard cols.count >= 2 else { return nil }
                return DictEntry(
                    file: file, line: index, reading: String(cols[0]), word: String(cols[1]))
            }
        }
    }

    func add(reading: String, word: String) {
        var kept = lines(of: Self.registered)
        kept.append("\(reading)\t\(word)\t名詞\tmimir登録")
        write(kept, to: Self.registered)
    }

    func remove(_ doomed: [DictEntry]) {
        for file in Set(doomed.map(\.file)) {
            let drop = Set(doomed.filter { $0.file == file }.map(\.line))
            let kept = lines(of: file).enumerated().filter { !drop.contains($0.offset) }.map(\.element)
            write(kept, to: file)
        }
    }

    /// Non-empty lines, indexed the way `DictEntry.line` counts them.
    private func lines(of file: String) -> [String] {
        let url = Self.dir.appendingPathComponent(file)
        let text = (try? String(contentsOf: url, encoding: .utf8)) ?? ""
        return text.split(separator: "\n").map(String.init)
    }

    private func write(_ lines: [String], to file: String) {
        try? FileManager.default.createDirectory(at: Self.dir, withIntermediateDirectories: true)
        let text = lines.isEmpty ? "" : lines.joined(separator: "\n") + "\n"
        try? text.write(to: Self.dir.appendingPathComponent(file), atomically: true, encoding: .utf8)
        load()
        EngineRestart.schedule()
    }
}

// MARK: - Learning history (held by the running engine)

struct LearnedEntry: Identifiable, Hashable {
    var id: String { reading + "\t" + surface }
    let reading: String
    let surface: String
    let frequency: Int
}

final class LearningHistory: ObservableObject {
    @Published var entries: [LearnedEntry] = []

    func load() {
        guard
            let data = engineClient.sendRequestSync(method: "learning_list", params: [:], timeout: 2),
            let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let raw = json["entries"] as? [[String: Any]]
        else { return }
        entries = raw.map {
            LearnedEntry(
                reading: $0["reading"] as? String ?? "", surface: $0["surface"] as? String ?? "",
                frequency: $0["frequency"] as? Int ?? 0)
        }
    }

    func remove(_ doomed: [LearnedEntry]) {
        for entry in doomed {
            _ = engineClient.sendRequestSync(
                method: "learning_remove",
                params: ["reading": entry.reading, "surface": entry.surface], timeout: 2)
        }
        load()
    }

    func clear() {
        _ = engineClient.sendRequestSync(method: "learning_clear", params: [:], timeout: 2)
        load()
    }
}

// MARK: - Views

struct GeneralTab: View {
    @ObservedObject var model: SettingsModel

    var body: some View {
        Form {
            Toggle("SKK 式の入力（大文字で漢字の変換を始め、小文字は平仮名で確定）", isOn: $model.skkStyle)
                .onChange(of: model.skkStyle) { model.set("conversion", "skk_style", $0) }
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
            Toggle("きょう・いま などを日付・時刻に変換する", isOn: $model.date)
                .onChange(of: model.date) { model.set("date", "enabled", $0) }
        }
        .formStyle(.grouped)
    }
}

struct ModelTab: View {
    @ObservedObject var model: SettingsModel
    @StateObject private var profiler = ModelProfiler()
    @State private var newProfile = ""

    var body: some View {
        Form {
            Section("組み合わせ") {
                Picker("プロファイル", selection: profileBinding) {
                    Text("（保存していない組み合わせ）").tag("")
                    ForEach(model.profiles.keys.sorted(), id: \.self) { Text($0).tag($0) }
                }
                Picker("メイン", selection: $model.model) {
                    ForEach(model.models, id: \.self) { Text($0).tag($0) }
                }
                .onChange(of: model.model) { model.set("conversion", "model", $0) }
                Picker("軽量（ビーム・自動切替）", selection: $model.lightModel) {
                    ForEach(model.models, id: \.self) { Text($0).tag($0) }
                }
                .onChange(of: model.lightModel) { model.set("conversion", "light_model", $0) }
                HStack {
                    TextField("新しいプロファイル", text: $newProfile, prompt: Text("名前"))
                    Button("今の組み合わせを保存") {
                        model.saveProfile(newProfile)
                        newProfile = ""
                    }
                    .disabled(newProfile.isEmpty || newProfile.contains("."))
                    if let current = model.currentProfile {
                        Button("「\(current)」を削除") { model.deleteProfile(current) }
                    }
                }
            }
            Section {
                if profiler.scores.isEmpty {
                    Text(profiler.running ? "計測中…（数秒〜十数秒）" : "各モデルの精度と速さを、この Mac で測ります。")
                        .foregroundStyle(.secondary)
                } else {
                    Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 8) {
                        GridRow {
                            Text("モデル")
                            Text("精度").gridColumnAlignment(.trailing)
                            Text("1回あたり").gridColumnAlignment(.trailing)
                            Text("読込").gridColumnAlignment(.trailing)
                        }
                        .font(.caption.weight(.semibold))
                        Divider()
                        ForEach(profiler.scores) { score in
                            GridRow {
                                Text(score.id)
                                    .fontWeight(score.id == model.model ? .semibold : .regular)
                                if let error = score.error {
                                    Text(error).foregroundStyle(.secondary).gridCellColumns(3)
                                } else {
                                    Text(String(format: "%.0f%%", (score.accuracy ?? 0) * 100))
                                    Text(String(format: "%.1f ms", score.avgMs ?? 0))
                                    Text(String(format: "%.0f ms", score.loadMs ?? 0))
                                }
                            }
                            .monospacedDigit()
                        }
                    }
                    Text("対象: \(profiler.samples)").font(.footnote).foregroundStyle(.secondary)
                }
            } header: {
                HStack {
                    Text("計測")
                    Spacer()
                    Button(profiler.running ? "計測中…" : "計測する") { profiler.run() }
                        .disabled(profiler.running)
                }
            }
        }
        .formStyle(.grouped)
    }

    private var profileBinding: Binding<String> {
        Binding(
            get: { model.currentProfile ?? "" },
            set: { if !$0.isEmpty { model.applyProfile($0) } })
    }
}

struct DictionaryTab: View {
    @StateObject private var dict = UserDictionary()
    @State private var selection = Set<DictEntry.ID>()
    @State private var query = ""
    @State private var reading = ""
    @State private var word = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                TextField("よみ（ひらがな）", text: $reading).frame(width: 160)
                TextField("単語", text: $word)
                Button("追加") {
                    dict.add(reading: reading, word: word)
                    reading = ""
                    word = ""
                }
                .disabled(reading.isEmpty || word.isEmpty)
            }
            Table(filtered, selection: $selection) {
                TableColumn("よみ", value: \.reading)
                TableColumn("単語", value: \.word)
                TableColumn("ファイル", value: \.file)
            }
            HStack {
                TextField("検索", text: $query).frame(width: 180)
                Text("\(filtered.count) 件").foregroundStyle(.secondary).monospacedDigit()
                Spacer()
                Button("Finder で開く") { NSWorkspace.shared.open(UserDictionary.dir) }
                Button("選択を削除") {
                    dict.remove(dict.entries.filter { selection.contains($0.id) })
                    selection = []
                }
                .disabled(selection.isEmpty)
            }
            if !dict.binaries.isEmpty {
                Text("バイナリ辞書（ここでは編集できません）: \(dict.binaries.joined(separator: ", "))")
                    .font(.footnote).foregroundStyle(.secondary)
            }
        }
        .padding(20)
        .onAppear { dict.load() }
    }

    private var filtered: [DictEntry] {
        query.isEmpty
            ? dict.entries
            : dict.entries.filter { $0.reading.contains(query) || $0.word.contains(query) }
    }
}

struct LearningTab: View {
    @StateObject private var history = LearningHistory()
    @State private var selection = Set<LearnedEntry.ID>()
    @State private var query = ""
    @State private var confirmReset = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Table(filtered, selection: $selection) {
                TableColumn("よみ", value: \.reading)
                TableColumn("確定した語", value: \.surface)
                TableColumn("回数") { Text("\($0.frequency)").monospacedDigit() }.width(48)
            }
            HStack {
                TextField("検索", text: $query).frame(width: 180)
                Text("\(filtered.count) 件").foregroundStyle(.secondary).monospacedDigit()
                Spacer()
                Button("選択を削除") {
                    history.remove(history.entries.filter { selection.contains($0.id) })
                    selection = []
                }
                .disabled(selection.isEmpty)
                Button("すべてリセット…") { confirmReset = true }
                    .disabled(history.entries.isEmpty)
            }
        }
        .padding(20)
        .onAppear { history.load() }
        .confirmationDialog("学習をすべて消しますか？", isPresented: $confirmReset) {
            Button("すべて消す", role: .destructive) { history.clear() }
        } message: {
            Text("\(history.entries.count) 件の学習が消え、元に戻せません。")
        }
    }

    private var filtered: [LearnedEntry] {
        query.isEmpty
            ? history.entries
            : history.entries.filter { $0.reading.contains(query) || $0.surface.contains(query) }
    }
}

struct SettingsView: View {
    @ObservedObject var model: SettingsModel

    var body: some View {
        VStack(spacing: 0) {
            TabView {
                GeneralTab(model: model).tabItem { Text("一般") }
                ModelTab(model: model).tabItem { Text("モデル") }
                DictionaryTab().tabItem { Text("辞書") }
                LearningTab().tabItem { Text("学習") }
            }
            if let error = model.error {
                Text(error).foregroundStyle(Color(nsColor: Swiss.accent)).padding(8)
            }
        }
        .padding(.top, 8)
        .frame(width: 600, height: 520)
    }
}

/// The settings window, opened from the input menu. The IME runs as a
/// background-only process, so it becomes an accessory app while the window
/// is open (to take keyboard focus) and goes back when it closes.
final class SettingsWindowController: NSObject, NSWindowDelegate {
    static let shared = SettingsWindowController()

    private let model = SettingsModel()
    private lazy var window: NSWindow = {
        let window = NSWindow(
            contentViewController: NSHostingController(rootView: SettingsView(model: model)))
        window.title = "Mimir 設定"
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
