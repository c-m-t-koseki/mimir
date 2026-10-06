import Cocoa
import SwiftUI

// MARK: - Engine-backed settings

/// The settings the window edits, read from and written to config.toml
/// through the engine (`get_config` / `set_config`). Changes restart the
/// engine, which is what applies them (settings load at `init`).
final class SettingsModel: ObservableObject {
    @Published var partHint = "none"
    @Published var candidateLayout = "vertical"
    @Published var candidateWindow = "always"
    @Published var readingDisplay = "tail"
    @Published var tailDelayMs = 250
    @Published var tailDeleteDelayMs = 400
    @Published var rewriteHoldMs = 250
    @Published var commitGuardMs = 0
    @Published var autoCommitMs = 3000
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
    /// `[keys]`: action → its keys, as written.
    @Published var keys: [String: String] = [:]

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
        candidateLayout = config["candidate_layout"] as? String ?? candidateLayout
        candidateWindow = config["candidate_window"] as? String ?? candidateWindow
        readingDisplay = config["reading_display"] as? String ?? readingDisplay
        tailDelayMs = config["tail_delay_ms"] as? Int ?? tailDelayMs
        tailDeleteDelayMs = config["tail_delete_delay_ms"] as? Int ?? tailDeleteDelayMs
        rewriteHoldMs = config["rewrite_hold_ms"] as? Int ?? rewriteHoldMs
        commitGuardMs = config["commit_guard_ms"] as? Int ?? commitGuardMs
        autoCommitMs = config["auto_commit_ms"] as? Int ?? autoCommitMs
        predictive = config["predictive"] as? Bool ?? predictive
        skkStyle = config["skk_style"] as? Bool ?? skkStyle
        model = config["model"] as? String ?? model
        lightModel = config["light_model"] as? String ?? lightModel
        learning = config["learning"] as? Bool ?? learning
        date = config["date"] as? Bool ?? date
        models = config["models"] as? [String] ?? models
        let rawProfiles = config["profiles"] as? [String: [String: String]] ?? [:]
        profiles = rawProfiles.mapValues { ($0["model"] ?? "", $0["light_model"] ?? "") }
        keys = config["keys"] as? [String: String] ?? keys
        saved = [
            "display.part_hint": partHint, "display.candidate_layout": candidateLayout,
            "display.candidate_window": candidateWindow,
            "display.reading_display": readingDisplay, "display.tail_delay_ms": "\(tailDelayMs)",
            "display.tail_delete_delay_ms": "\(tailDeleteDelayMs)",
            "display.rewrite_hold_ms": "\(rewriteHoldMs)",
            "display.commit_guard_ms": "\(commitGuardMs)",
            "display.auto_commit_ms": "\(autoCommitMs)",
            "conversion.predictive": "\(predictive)",
            "conversion.skk_style": "\(skkStyle)",
            "conversion.model": model, "conversion.light_model": lightModel,
            "learning.enabled": "\(learning)", "date.enabled": "\(date)",
        ]
        for (action, value) in keys {
            saved["keys.\(action)"] = value
        }
        error = nil
    }

    /// Write `[section] key = value` (`nil` removes it). Returns success.
    @discardableResult
    func set(_ section: String, _ key: String, _ value: Any?) -> Bool {
        let id = "\(section).\(key)"
        let text = value.map { "\($0)" }
        guard saved[id] != text else { return true }
        let params: [String: Any] = ["section": section, "key": key, "value": value ?? NSNull()]
        guard let data = engineClient.sendRequestSync(method: "set_config", params: params, timeout: 2)
        else {
            error = "[\(section)] \(key) を保存できませんでした"
            load()
            return false
        }
        saved[id] = text
        error = nil
        // Keys and display settings apply at once; the rest (models,
        // dictionaries, learning) take an engine restart.
        let reply = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        if reply?["restart"] as? Bool ?? true {
            EngineRestart.schedule()
        }
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

// MARK: - Learning and completion history (held by the running engine)

struct LearnedEntry: Identifiable, Hashable {
    var id: String { reading + "\t" + surface }
    let reading: String
    let surface: String
    let frequency: Int
    let lastAccess: Date
}

/// One of the engine's two conversion stores: `learning` (what reranks
/// conversion) or `history` (every commit, what completion draws from).
/// The store name prefixes its methods (`learning_list`, `history_list`, …).
final class LearningHistory: ObservableObject {
    @Published var entries: [LearnedEntry] = []
    let store: String

    init(store: String) { self.store = store }

    func load() {
        guard
            let data = engineClient.sendRequestSync(method: "\(store)_list", params: [:], timeout: 2),
            let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let raw = json["entries"] as? [[String: Any]]
        else { return }
        entries = raw.map {
            LearnedEntry(
                reading: $0["reading"] as? String ?? "", surface: $0["surface"] as? String ?? "",
                frequency: $0["frequency"] as? Int ?? 0,
                lastAccess: Date(timeIntervalSince1970: $0["last_access"] as? Double ?? 0))
        }
    }

    func remove(_ doomed: [LearnedEntry]) {
        for entry in doomed {
            _ = engineClient.sendRequestSync(
                method: "\(store)_remove",
                params: ["reading": entry.reading, "surface": entry.surface], timeout: 2)
        }
        load()
    }

    func clear() {
        _ = engineClient.sendRequestSync(method: "\(store)_clear", params: [:], timeout: 2)
        load()
    }
}

// MARK: - Views

/// The general settings, in three groups: how input goes (操作), what
/// shows while typing (表示), and what conversion offers and keeps (変換).
struct GeneralTab: View {
    @ObservedObject var model: SettingsModel

    var body: some View {
        Form {
            Section("操作") {
                Toggle("SKK 式の入力（大文字で漢字の変換を始め、小文字は平仮名で確定）", isOn: $model.skkStyle)
                    .onChange(of: model.skkStyle) { model.set("conversion", "skk_style", $0) }
                Stepper(
                    model.autoCommitMs == 0
                        ? "手を止めたら確定: しない" : "手を止めたら確定: \(model.autoCommitMs) ミリ秒後",
                    value: $model.autoCommitMs, in: 0...5000, step: 100
                )
                .onChange(of: model.autoCommitMs) { model.set("display", "auto_commit_ms", $0) }
            }
            Section("表示") {
                Picker("変換中の読み", selection: $model.readingDisplay) {
                    Text("末尾1字を平仮名で残す").tag("tail")
                    Text("ふりがな（最後の漢字の上）").tag("ruby")
                    Text("読みを添える（天気〈てんき〉）").tag("inline")
                    Text("表示しない").tag("none")
                }
                .onChange(of: model.readingDisplay) { model.set("display", "reading_display", $0) }
                if model.readingDisplay == "tail" {
                    Stepper(
                        "手を止めてから変換するまで: \(model.tailDelayMs) ミリ秒", value: $model.tailDelayMs,
                        in: 50...1000, step: 50
                    )
                    .onChange(of: model.tailDelayMs) { model.set("display", "tail_delay_ms", $0) }
                    Stepper(
                        "削除のあと変換するまで: \(model.tailDeleteDelayMs) ミリ秒",
                        value: $model.tailDeleteDelayMs, in: 50...2000, step: 50
                    )
                    .onChange(of: model.tailDeleteDelayMs) { model.set("display", "tail_delete_delay_ms", $0) }
                    Stepper(
                        "変換して出したあと、平仮名に戻さない: \(model.rewriteHoldMs) ミリ秒",
                        value: $model.rewriteHoldMs, in: 0...1000, step: 50
                    )
                    .onChange(of: model.rewriteHoldMs) { model.set("display", "rewrite_hold_ms", $0) }
                    Stepper(
                        "変換して出した直後の確定は、前の表示で: \(model.commitGuardMs) ミリ秒",
                        value: $model.commitGuardMs, in: 0...1000, step: 50
                    )
                    .onChange(of: model.commitGuardMs) { model.set("display", "commit_guard_ms", $0) }
                }
                Picker("候補窓を出す", selection: $model.candidateWindow) {
                    Text("変換を始めてから（Ctrl+N など）").tag("conversion")
                    Text("入力中から常に").tag("always")
                }
                .onChange(of: model.candidateWindow) { model.set("display", "candidate_window", $0) }
                Picker("候補窓の並び", selection: $model.candidateLayout) {
                    Text("縦に一列").tag("vertical")
                    Text("キーの位置どおり（2段）").tag("keyboard")
                }
                .onChange(of: model.candidateLayout) { model.set("display", "candidate_layout", $0) }
                Toggle(
                    "補完候補を表示する（Tab で補完）",
                    isOn: Binding(
                        get: { model.partHint == "completion" },
                        set: { model.partHint = $0 ? "completion" : "none" })
                )
                .onChange(of: model.partHint) { model.set("display", "part_hint", $0) }
            }
            Section("変換") {
                Toggle("予測候補を候補窓に出す", isOn: $model.predictive)
                    .onChange(of: model.predictive) { model.set("conversion", "predictive", $0) }
                Toggle("変換結果を学習する", isOn: $model.learning)
                    .onChange(of: model.learning) { model.set("learning", "enabled", $0) }
                Toggle("きょう・いま などを日付・時刻に変換する", isOn: $model.date)
                    .onChange(of: model.date) { model.set("date", "enabled", $0) }
            }
        }
        .formStyle(.grouped)
    }
}

/// Captures the next key presses in the settings window while armed,
/// turning each into the `[keys]` notation (C-n, S-Tab, Space, q).
final class KeyRecorder: ObservableObject {
    /// The action being recorded, `nil` when idle.
    @Published private(set) var action: String?
    /// Keys taken so far (the select keys take several).
    @Published private(set) var taken: [String] = []
    private var monitor: Any?
    private var wanted = 1
    private var done: (([String]) -> Void)?

    /// Take `count` keys for `action`, then hand them to `done`.
    func record(_ action: String, count: Int = 1, done: @escaping ([String]) -> Void) {
        stop()
        self.action = action
        wanted = count
        self.done = done
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, let spec = Self.spec(event, charactersOnly: count > 1) else { return nil }
            self.taken.append(spec)
            if self.taken.count >= self.wanted {
                let keys = self.taken
                let done = self.done
                self.stop()
                done?(keys)
            }
            return nil
        }
    }

    func stop() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
        action = nil
        taken = []
        done = nil
    }

    private static let named: [UInt16: String] = [
        49: "Space", 48: "Tab", 36: "Return", 76: "Return", 53: "Escape", 51: "BackSpace",
        117: "Delete", 123: "Left", 124: "Right", 125: "Down", 126: "Up", 115: "Home",
        119: "End", 116: "PageUp", 121: "PageDown",
    ]

    /// `event` in the `[keys]` notation; `nil` for what can't be bound
    /// (⌘/⌥ chords, dead keys) — or, `charactersOnly`, anything but a plain
    /// character.
    static func spec(_ event: NSEvent, charactersOnly: Bool) -> String? {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if flags.contains(.command) || flags.contains(.option) { return nil }
        let ctrl = flags.contains(.control)
        let shift = flags.contains(.shift)
        if let name = named[event.keyCode] {
            if charactersOnly { return nil }
            return (ctrl ? "C-" : "") + (shift ? "S-" : "") + name
        }
        guard let chars = event.charactersIgnoringModifiers, chars.count == 1,
            let c = chars.first, c.isASCII, !c.isWhitespace
        else { return nil }
        if charactersOnly { return ctrl ? nil : String(c) }
        // A letter with Shift is S-a; other characters carry their shift
        // (? rather than S-/).
        if c.isLetter {
            return (ctrl ? "C-" : "") + (shift ? "S-" : "") + c.lowercased()
        }
        return (ctrl ? "C-" : "") + String(c)
    }
}

/// `[keys]`: each action's keys as chips; × drops one, ＋ records the next
/// key pressed. The select keys are recorded afresh, eight in a row. A
/// binding the engine refuses shows the old one again.
struct KeysTab: View {
    @ObservedObject var model: SettingsModel
    @StateObject private var recorder = KeyRecorder()

    private static let actions: [(String, String)] = [
        ("convert", "候補窓を開く"), ("next", "次の候補"), ("prev", "前の候補"),
        ("close", "パートを閉じて確定"), ("commit", "すべて確定"), ("cancel", "取り消す"),
        ("katakana", "カタカナで確定"), ("complete", "以前の入力で補完"),
        ("left", "変換範囲を縮める"), ("right", "変換範囲を伸ばす"), ("home", "先頭の1文字だけ"),
        ("end", "全体"), ("end_commit", "末尾ですべて確定"), ("register", "単語登録"), ("discard", "入力を捨てる"),
        ("kana", "かな入力へ"), ("ascii", "英数（直接入力）へ"), ("abbrev", "英字のまま変換"),
    ]

    private func keys(_ action: String) -> [String] {
        (model.keys[action] ?? "").split(separator: " ").map(String.init)
    }

    private func save(_ action: String, _ keys: [String]) {
        let value = keys.joined(separator: " ")
        model.keys[action] = value
        if !model.set("keys", action, value) {
            model.load()
        }
    }

    var body: some View {
        Form {
            Section {
                HStack {
                    Text("候補を選ぶキー")
                    Spacer()
                    if recorder.action == "select" {
                        Text("キーを8つ順に押す… \(recorder.taken.joined())").foregroundStyle(.secondary)
                        Button("やめる") { recorder.stop() }
                    } else {
                        Text(model.keys["select"] ?? "").font(.system(.body, design: .monospaced))
                        Button("記録し直す") {
                            recorder.record("select", count: 8) { save("select", [$0.joined()]) }
                        }
                    }
                }
            }
            Section {
                ForEach(Self.actions, id: \.0) { action, label in
                    HStack {
                        Text(label)
                        Spacer()
                        ForEach(keys(action), id: \.self) { key in
                            KeyChip(key: key) { save(action, keys(action).filter { $0 != key }) }
                        }
                        if recorder.action == action {
                            Text("キーを押す…").foregroundStyle(.secondary)
                            Button("やめる") { recorder.stop() }
                        } else {
                            Button {
                                recorder.record(action) { taken in
                                    save(action, keys(action).filter { !taken.contains($0) } + taken)
                                }
                            } label: {
                                Image(systemName: "plus")
                            }
                            .buttonStyle(.borderless)
                        }
                    }
                }
            } footer: {
                Text("＋ を押してから、割り当てたいキーを押す。× で外す。")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .onDisappear { recorder.stop() }
    }
}

/// One bound key, shown the way the keyboard names it, with × to drop it.
struct KeyChip: View {
    let key: String
    let remove: () -> Void

    /// C-n → ⌃N, S-Tab → ⇧Tab.
    private var shown: String {
        var rest = Substring(key)
        var prefix = ""
        while true {
            if rest.hasPrefix("C-"), rest.count > 2 { prefix += "⌃"; rest = rest.dropFirst(2) }
            else if rest.hasPrefix("S-"), rest.count > 2 { prefix += "⇧"; rest = rest.dropFirst(2) }
            else { break }
        }
        let names = [
            "Space": "Space", "Return": "↩", "Escape": "Esc", "Tab": "⇥", "BackSpace": "⌫",
            "Delete": "⌦", "Left": "←", "Right": "→", "Up": "↑", "Down": "↓", "Home": "Home",
            "End": "End", "PageUp": "PgUp", "PageDown": "PgDn",
        ]
        let name = names[String(rest)] ?? (rest.count == 1 ? rest.uppercased() : String(rest))
        return prefix + name
    }

    var body: some View {
        HStack(spacing: 4) {
            Text(shown).font(.system(.callout, design: .monospaced))
            Button(action: remove) { Image(systemName: "xmark").font(.caption2) }
                .buttonStyle(.borderless)
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 2)
        .overlay(RoundedRectangle(cornerRadius: 2).stroke(Color.secondary.opacity(0.4)))
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

/// The 学習 and 履歴 tabs: one store's entries, newest first by default.
struct LearningTab: View {
    @StateObject private var history: LearningHistory
    /// What the store holds, for the reset dialog (学習 / 履歴).
    let noun: String
    @State private var selection = Set<LearnedEntry.ID>()
    @State private var query = ""
    @State private var confirmReset = false
    @State private var sortOrder = [KeyPathComparator(\LearnedEntry.lastAccess, order: .reverse)]

    init(store: String, noun: String) {
        _history = StateObject(wrappedValue: LearningHistory(store: store))
        self.noun = noun
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Table(filtered.sorted(using: sortOrder), selection: $selection, sortOrder: $sortOrder) {
                TableColumn("よみ", value: \.reading)
                TableColumn("確定した語", value: \.surface)
                TableColumn("回数", value: \.frequency) { Text("\($0.frequency)").monospacedDigit() }
                    .width(48)
                TableColumn("最後に確定", value: \.lastAccess) {
                    Text($0.lastAccess, format: .dateTime.year().month().day().hour().minute())
                        .monospacedDigit()
                }
                .width(120)
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
        .confirmationDialog("\(noun)をすべて消しますか？", isPresented: $confirmReset) {
            Button("すべて消す", role: .destructive) { history.clear() }
        } message: {
            Text("\(history.entries.count) 件の\(noun)が消え、元に戻せません。")
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
                KeysTab(model: model).tabItem { Text("キー") }
                ModelTab(model: model).tabItem { Text("モデル") }
                DictionaryTab().tabItem { Text("辞書") }
                LearningTab(store: "learning", noun: "学習").tabItem { Text("学習") }
                LearningTab(store: "history", noun: "履歴").tabItem { Text("履歴") }
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
