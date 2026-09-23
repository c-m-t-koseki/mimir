import Cocoa
import InputMethodKit

/// Thin InputMethodKit adapter for the mimir engine.
///
/// All IME state (Empty → Composing → Conversion, romaji conversion,
/// candidates, learning) lives in mimir-imserver; this controller only
/// translates key events and applies the resulting UI actions, mirroring
/// the fcitx5 addon (mimir.cpp).
@objc(MimirInputController)
class MimirInputController: IMKInputController {
    static let candidateWindow = CandidateWindowController()
    static let modeBadge = ModeBadgeController()
    static let readingPanel = ReadingPanelController()
    /// Last input mode the engine reported; a change shows the badge.
    static var lastMode = "kana"

    /// Mirrors whether the engine currently shows a preedit (updated from
    /// engine actions). Used to decide when to refresh surrounding text.
    private var hasPreedit = false

    /// What this client was sent since the last line break or cursor jump:
    /// the conversion context for apps that can't report the text left of
    /// the cursor (terminals — iTerm2, Terminal, Vim inside them). One
    /// controller per client, so each app keeps its own.
    private var typed = TypedHistory()

    /// Detects the lone right-⌘ tap that returns to hiragana mode on
    /// keyboards without a JIS かな key (issue #33).
    private var rightCommandTap = RightCommandTapDetector()

    // MARK: - Event handling

    override func recognizedEvents(_ sender: Any!) -> Int {
        // flagsChanged is needed for the right-⌘ tap detection; plain
        // keyDown events never carry lone-modifier transitions.
        Int(NSEvent.EventTypeMask([.keyDown, .flagsChanged]).rawValue)
    }

    override func handle(_ event: NSEvent!, client sender: Any!) -> Bool {
        guard let event else { return false }

        // Lone right-⌘ tap → hiragana, the US-layout stand-in for the JIS
        // かな key. Never consume flagsChanged: the system must keep an
        // accurate view of modifier state.
        if event.type == .flagsChanged {
            let fired = rightCommandTap.handleFlagsChanged(
                keyCode: event.keyCode,
                flags: event.modifierFlags.intersection(.deviceIndependentFlagsMask)
            )
            if fired, let client = sender as? (any IMKTextInput) {
                sendKanaToggle(client: client)
            }
            return false
        }

        guard event.type == .keyDown else { return false }
        guard let client = sender as? (any IMKTextInput) else { return false }

        // A real key press means any held right ⌘ is a shortcut modifier,
        // not a pending tap.
        rightCommandTap.cancel()

        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        // Never swallow Command shortcuts.
        if flags.contains(.command) { return false }

        // JIS かな key (and Karabiner right-Command tap → かな): always
        // consume so the system doesn't process keyCode 104 after the engine
        // returns not_consumed (already in hiragana mode).
        if event.keyCode == KeyCodeMap.kanaKeyCode {
            sendKanaToggle(client: client)
            return true
        }

        // JIS 英数 key (and Karabiner's left-⌘ tap → 英数): Mimir's own
        // ASCII mode, not the OS English input source. Consumed like かな,
        // so the system never switches sources on it.
        if event.keyCode == KeyCodeMap.eisuKeyCode {
            let key = EngineKeyEvent(keysym: KeyCodeMap.eisuKeysym, modifiers: KeyModifiers())
            if let result = engineClient.processKeySync(key) {
                apply(actions: result.actions, client: client)
                showModeChange(result.mode, client: client)
                showHint(result.hint, client: client)
            }
            return true
        }

        guard let key = KeyCodeMap.translate(event: event) else { return false }

        // Refresh the conversion context while no composition is active
        // (mirrors the fcitx5 addon, which captures surrounding text in the
        // Empty state). Queued before process_key on the same pipe, so the
        // engine sees it first. Skipped for function/navigation keysyms
        // (0xff00 range): they can't start a composition, and the three
        // synchronous client IPCs in sendSurroundingText would otherwise
        // fire on every arrow-key repeat.
        if !hasPreedit && key.keysym < 0xff00 {
            sendSurroundingText(client: client)
        }

        guard let result = engineClient.processKeySync(key) else {
            // Engine busy or dead: let the key pass through rather than
            // freezing input.
            return false
        }
        apply(actions: result.actions, client: client)
        showModeChange(result.mode, client: client)
        showHint(result.hint, client: client)
        if !result.consumed {
            typed.passedThrough(key)
        }
        return result.consumed
    }

    private func showHint(_ hint: String?, client: any IMKTextInput) {
        guard let hint, !Self.candidateWindow.isVisible else {
            Self.readingPanel.hide()
            return
        }
        var rect: NSRect?
        if !Self.readingPanel.isVisible {
            var lineRect = NSRect.zero
            client.attributes(forCharacterIndex: 0, lineHeightRectangle: &lineRect)
            rect = lineRect
        }
        Self.readingPanel.show(hint, cursorRect: rect)
    }

    static let japaneseModeID = "dev.tkoseki.inputmethod.Mimir.Japanese"
    static let romanModeID = "dev.tkoseki.inputmethod.Mimir.Roman"

    /// The engine switched modes: move the menu bar icon (か / A) to the
    /// matching input mode, and show the badge. macOS shows its own indicator
    /// only when it switched the mode itself (`badge: false`); our
    /// `selectMode` gets none.
    private func showModeChange(_ mode: String?, client: any IMKTextInput, badge: Bool = true) {
        guard let mode, mode != Self.lastMode else { return }
        Self.lastMode = mode
        if badge { showBadge(client: client) }
        client.selectMode(mode == "ascii" ? Self.romanModeID : Self.japaneseModeID)
    }

    private func showBadge(client: any IMKTextInput) {
        var rect = NSRect.zero
        client.attributes(forCharacterIndex: 0, lineHeightRectangle: &rect)
        Self.modeBadge.show(mode: Self.lastMode, near: rect)
    }

    /// macOS switched the input mode (the input menu, or our own
    /// `selectMode`): bring the engine along when it isn't there yet.
    override func setValue(_ value: Any!, forTag tag: Int, client sender: Any!) {
        guard tag == kTextServiceInputModePropertyTag, let modeID = value as? String,
            let client = sender as? (any IMKTextInput)
        else {
            super.setValue(value, forTag: tag, client: sender)
            return
        }
        let wantASCII = modeID == Self.romanModeID
        guard wantASCII != (Self.lastMode == "ascii") else { return }
        let keysym = wantASCII ? KeyCodeMap.eisuKeysym : KeyCodeMap.superRKeysym
        if let result = engineClient.processKeySync(
            EngineKeyEvent(keysym: keysym, modifiers: KeyModifiers()))
        {
            apply(actions: result.actions, client: client)
            showModeChange(result.mode, client: client, badge: false)
        }
    }

    /// Send the return-to-hiragana toggle (Super_R) to the engine and apply
    /// the resulting actions.
    private func sendKanaToggle(client: any IMKTextInput) {
        let key = EngineKeyEvent(keysym: KeyCodeMap.superRKeysym, modifiers: KeyModifiers())
        if let result = engineClient.processKeySync(key) {
            apply(actions: result.actions, client: client)
            showModeChange(result.mode, client: client)
            showHint(result.hint, client: client)
        }
    }

    // MARK: - Input menu

    override func menu() -> NSMenu! {
        let menu = NSMenu(title: "Mimir")
        menu.addItem(
            NSMenuItem(title: "Mimir の設定…", action: #selector(openSettings(_:)), keyEquivalent: ""))
        return menu
    }

    @objc func openSettings(_ sender: Any?) {
        SettingsWindowController.shared.show()
    }

    // MARK: - Lifecycle

    /// Focus came to a client: keep its input mode (the menu bar icon) in
    /// step with the engine's, which is shared by every app.
    override func activateServer(_ sender: Any!) {
        super.activateServer(sender)
        guard let client = sender as? (any IMKTextInput) else { return }
        client.selectMode(Self.lastMode == "ascii" ? Self.romanModeID : Self.japaneseModeID)
    }

    override func deactivateServer(_ sender: Any!) {
        // A right-⌘ press armed before a focus switch must not fire after
        // it (e.g. right-⌘-clicking another window).
        rightCommandTap.cancel()
        // Mozc-style: commit the pending preedit on focus loss, then
        // persist what the user taught us.
        if let client = sender as? (any IMKTextInput) {
            flushComposition(client: client)
        } else {
            Self.candidateWindow.hide()
        }
        engineClient.saveLearningAsync()
        super.deactivateServer(sender)
    }

    override func commitComposition(_ sender: Any!) {
        if let client = sender as? (any IMKTextInput) {
            flushComposition(client: client)
        } else {
            Self.candidateWindow.hide()
        }
    }

    /// Commit any pending composition via the engine and apply the cleanup
    /// actions it emits (clear preedit, hide candidates/aux).
    private func flushComposition(client: any IMKTextInput) {
        Self.readingPanel.hide()
        if let result = engineClient.commitSync() {
            apply(actions: result.actions, client: client)
        } else {
            // Engine unavailable: still drop any stale candidate panel.
            Self.candidateWindow.hide()
        }
    }

    // MARK: - Applying engine actions

    private func apply(actions: [EngineAction], client: any IMKTextInput) {
        // The engine emits ShowCandidates before UpdateAux. Fold aux changes
        // in first (deferring their render when a candidate update follows)
        // so the panel is rendered once per batch, not once for the
        // candidates and again for the aux footer.
        let updatesCandidates = actions.contains {
            switch $0 {
            case .showCandidates, .hideCandidates: return true
            default: return false
            }
        }
        for action in actions {
            switch action {
            case .updateAux(let text):
                Self.candidateWindow.setAux(text, deferRender: updatesCandidates)
            case .hideAux:
                Self.candidateWindow.setAux(nil, deferRender: updatesCandidates)
            default:
                break
            }
        }

        for action in actions {
            switch action {
            case .commit(let text):
                // insertText replaces the marked text and ends the
                // composition; since #46 the engine no longer pairs Commit
                // with an empty UpdatePreedit, so clear the flag here or the
                // next keystroke would skip the surrounding-text refresh.
                hasPreedit = false
                typed.append(text)
                client.insertText(text, replacementRange: NSRange(location: NSNotFound, length: 0))

            case .updatePreedit(let text, let caret, let attributes):
                hasPreedit = !text.isEmpty
                setMarkedText(text: text, caret: caret, attributes: attributes, client: client)

            case .showCandidates(let candidates, let cursor, let page, let totalPages):
                // Query the composition anchor (a synchronous IPC into the
                // focused app) only when the panel comes on screen; it
                // doesn't move while the panel stays visible.
                var cursorRect: NSRect?
                if !Self.candidateWindow.isVisible {
                    var lineHeightRect = NSRect.zero
                    client.attributes(forCharacterIndex: 0, lineHeightRectangle: &lineHeightRect)
                    cursorRect = lineHeightRect
                }
                Self.candidateWindow.show(
                    candidates: candidates,
                    cursor: cursor,
                    page: page,
                    totalPages: totalPages,
                    cursorRect: cursorRect
                )

            case .hideCandidates:
                Self.candidateWindow.hide()

            case .updateAux, .hideAux:
                break  // applied above
            }
        }
    }

    /// Send the text left of the cursor to the engine as conversion
    /// context. Gated on `selectedRange` only: `client.length()` is the
    /// least-implemented part of IMKTextInput (it returns 0 even in apps
    /// whose `attributedSubstring` works fine), and the request below is
    /// capped to 40 UTF-16 units anyway, so document size doesn't matter.
    /// Whether a client supports this at all is app-dependent (Cocoa text
    /// views do; Electron/Chromium/terminals mostly don't), so the skip
    /// reasons are logged for dogfooding visibility.
    private func sendSurroundingText(client: any IMKTextInput) {
        // When capture isn't possible (terminals), send what this IME typed
        // into the client since the last line break or cursor jump
        // (`TypedHistory`) rather than skipping: leaving the context from a
        // previous cursor position in place would make the engine condition
        // on text that is no longer left of the cursor. The history forgets
        // itself on any key that may move the cursor, so it is empty rather
        // than wrong.
        let selected = client.selectedRange()
        guard selected.location != NSNotFound, selected.location > 0 else {
            NSLog("MimirIME: surrounding text unavailable in \(client.bundleIdentifier() ?? "?") (no usable selection), using typed history")
            sendTypedHistory()
            return
        }

        let maxContextUTF16 = 40  // engine truncates further per its config
        let start = max(0, selected.location - maxContextUTF16)
        let range = NSRange(location: start, length: selected.location - start)
        // string(from:actualRange:) rather than attributedSubstring(from:):
        // it's the IMKTextInput document-access method clients actually
        // implement (azooKey-Desktop settled on the same call).
        var actualRange = NSRange()
        guard let leftContext = client.string(from: range, actualRange: &actualRange),
            !leftContext.isEmpty
        else {
            NSLog("MimirIME: surrounding text unavailable in \(client.bundleIdentifier() ?? "?") (string(from:)), using typed history")
            sendTypedHistory()
            return
        }

        NSLog("MimirIME: surrounding text captured (\(leftContext.count) chars)")
        engineClient.setSurroundingTextAsync(
            text: leftContext,
            cursorPos: leftContext.unicodeScalars.count
        )
    }

    /// Fallback context: what this IME sent the client itself.
    private func sendTypedHistory() {
        engineClient.setSurroundingTextAsync(
            text: typed.text, cursorPos: typed.text.unicodeScalars.count)
    }

    private func setMarkedText(
        text: String, caret: Int, attributes: [PreeditAttr], client: any IMKTextInput
    ) {
        guard !text.isEmpty else {
            client.setMarkedText(
                NSAttributedString(string: ""),
                selectionRange: NSRange(location: 0, length: 0),
                replacementRange: NSRange(location: NSNotFound, length: 0)
            )
            return
        }

        let attributed = NSMutableAttributedString(
            string: text,
            attributes: [.underlineStyle: NSUnderlineStyle.single.rawValue]
        )
        for attr in attributes {
            guard let range = utf16Range(of: attr.start..<attr.end, in: text) else { continue }
            let style: NSUnderlineStyle
            switch attr.style {
            // The focused/highlighted segment is drawn with a thick
            // underline (the convention azooKey/mac-akaza use for marked
            // text, since background colors are unreliable across apps).
            case "underline_double", "highlight", "reverse":
                style = .thick
            default:
                style = .single
            }
            attributed.addAttribute(.underlineStyle, value: style.rawValue, range: range)
        }

        let caretUTF16 = utf16Offset(ofScalarOffset: caret, in: text)
        client.setMarkedText(
            attributed,
            selectionRange: NSRange(location: caretUTF16, length: 0),
            replacementRange: NSRange(location: NSNotFound, length: 0)
        )
    }
}

// MARK: - Unicode scalar → UTF-16 offset conversion

/// The engine reports positions in Unicode scalar values; IMK APIs take
/// UTF-16 offsets.
func utf16Offset(ofScalarOffset offset: Int, in text: String) -> Int {
    let scalars = text.unicodeScalars
    let clamped = min(max(offset, 0), scalars.count)
    let index = scalars.index(scalars.startIndex, offsetBy: clamped)
    return text.utf16.distance(from: text.utf16.startIndex, to: index)
}

func utf16Range(of scalarRange: Range<Int>, in text: String) -> NSRange? {
    guard scalarRange.lowerBound >= 0, scalarRange.lowerBound <= scalarRange.upperBound else {
        return nil
    }
    let start = utf16Offset(ofScalarOffset: scalarRange.lowerBound, in: text)
    let end = utf16Offset(ofScalarOffset: scalarRange.upperBound, in: text)
    return NSRange(location: start, length: end - start)
}

/// Text recently sent to a client, kept as a stand-in for the surrounding
/// text a terminal can't report. Anything that may move the cursor or start
/// a new line (Return, Escape, arrows, Ctrl chords) forgets it: a wrong
/// context is worse than none.
struct TypedHistory {
    private static let limit = 40
    private(set) var text = ""

    mutating func append(_ s: String) {
        text = String((text + s).suffix(Self.limit))
    }

    /// A key the engine let through to the application.
    mutating func passedThrough(_ key: EngineKeyEvent) {
        let m = key.modifiers
        if key.keysym == 0xff08 {  // BackSpace
            text = String(text.dropLast())
        } else if (0x20...0x7e).contains(key.keysym), !m.control, !m.alt, !m.superKey,
            let scalar = UnicodeScalar(key.keysym)
        {
            append(String(Character(scalar)))
        } else {
            text = ""
        }
    }
}
