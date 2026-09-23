import Cocoa

/// Swiss-style palette and type scale shared by the candidate window and
/// the mode badge: monochrome, one red accent, hierarchy by size and weight.
enum Swiss {
    static let accent = NSColor(srgbRed: 0.890, green: 0.024, blue: 0.075, alpha: 1)
    static let rule = NSColor.labelColor.withAlphaComponent(0.25)
    /// 8pt baseline grid.
    static let grid: CGFloat = 8
    static let candidateFont = NSFont.systemFont(ofSize: 17)
    static let selectedFont = NSFont.systemFont(ofSize: 17, weight: .semibold)
    static let labelFont = NSFont.monospacedSystemFont(ofSize: 11, weight: .medium)
    static let smallFont = NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .regular)

    /// A flat box with a hairline rule and a 2pt corner, colors resolved
    /// per appearance by AppKit.
    static func box(fill: NSColor = .textBackgroundColor) -> NSBox {
        let box = NSBox()
        box.boxType = .custom
        box.borderColor = rule
        box.borderWidth = 1
        box.cornerRadius = 2
        box.fillColor = fill
        box.titlePosition = .noTitle
        box.contentViewMargins = .zero
        return box
    }

    static func panel() -> NSPanel {
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 200, height: 100),
            styleMask: [.nonactivatingPanel, .borderless],
            backing: .buffered,
            defer: true
        )
        panel.level = .popUpMenu
        panel.hidesOnDeactivate = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.ignoresMouseEvents = true
        return panel
    }
}

/// Custom candidate window (borderless non-activating NSPanel).
///
/// The engine pre-paginates: `show` receives only the visible page plus
/// page metadata, so this controller just renders rows. Each row is
/// `label  candidate  description` on the 8pt grid; the selected row is
/// marked by a red bar at its left edge, never by a fill. Labels are the
/// keys that pick the row (`a s d f j k l`, see `part.rs`).
class CandidateWindowController {
    static let selectKeys = Array("asdfjkl")
    private static let rowHeight = Swiss.grid * 4
    private static let minPanelWidth: CGFloat = 160

    private let panel: NSPanel
    private let stackView: NSStackView
    private var rowViews: [NSView] = []
    private var auxText: String?

    private struct PageState {
        let candidates: [CandidateItem]
        let cursor: Int
        let page: Int
        let totalPages: Int
    }
    private var pageState: PageState?

    init() {
        panel = Swiss.panel()
        let box = Swiss.box()
        panel.contentView = box

        stackView = NSStackView()
        stackView.orientation = .vertical
        stackView.alignment = .leading
        stackView.spacing = 0
        stackView.edgeInsets = NSEdgeInsets(
            top: Swiss.grid, left: 0, bottom: Swiss.grid, right: Swiss.grid * 2)
        stackView.translatesAutoresizingMaskIntoConstraints = false

        let content = box.contentView!
        content.addSubview(stackView)
        NSLayoutConstraint.activate([
            stackView.topAnchor.constraint(equalTo: content.topAnchor),
            stackView.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            stackView.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            stackView.bottomAnchor.constraint(equalTo: content.bottomAnchor),
        ])
    }

    var isVisible: Bool { panel.isVisible }

    /// `cursorRect: nil` reuses the rect from the previous `show` — the
    /// caller can skip its (synchronous, per-keystroke) client IPC while
    /// the panel is already on screen, since the composition anchor
    /// doesn't move mid-composition.
    func show(
        candidates: [CandidateItem], cursor: Int, page: Int, totalPages: Int, cursorRect: NSRect?
    ) {
        pageState = PageState(
            candidates: candidates, cursor: cursor, page: page, totalPages: totalPages)
        render(cursorRect: cursorRect)
    }

    /// Update the aux footer; re-renders in place if the window is visible.
    /// Pass `deferRender: true` when a `show`/`hide` follows in the same
    /// action batch, so the panel is rendered once per batch instead of
    /// once for the aux change and again for the candidates.
    func setAux(_ text: String?, deferRender: Bool = false) {
        auxText = text
        if !deferRender, panel.isVisible, pageState != nil {
            render(cursorRect: nil)
        }
    }

    func hide() {
        pageState = nil
        panel.orderOut(nil)
    }

    private func render(cursorRect: NSRect?) {
        clearRows()
        guard let state = pageState else {
            hide()
            return
        }
        // An empty list still renders while the aux footer has text: a
        // source-filtered view narrowed to an empty source must show its
        // 「候補なし」 footer, not a silently vanishing panel.
        if state.candidates.isEmpty && (auxText ?? "").isEmpty {
            hide()
            return
        }

        for (index, candidate) in state.candidates.enumerated() {
            addRow(candidate, index: index, selected: index == state.cursor)
        }
        var footer: [String] = []
        if state.totalPages > 1 {
            footer.append("\(state.page + 1) / \(state.totalPages)")
        }
        if let aux = auxText, !aux.isEmpty {
            footer.append(aux)
        }
        if !footer.isEmpty {
            addFooter(footer.joined(separator: "   "))
        }

        positionPanel(cursorRect: cursorRect)
    }

    private func clearRows() {
        for view in rowViews {
            stackView.removeArrangedSubview(view)
            view.removeFromSuperview()
        }
        rowViews.removeAll()
    }

    private func addRow(_ candidate: CandidateItem, index: Int, selected: Bool) {
        let bar = NSBox()
        bar.boxType = .custom
        bar.borderWidth = 0
        bar.fillColor = selected ? Swiss.accent : .clear

        let key = index < Self.selectKeys.count ? String(Self.selectKeys[index]) : ""
        let label = NSTextField(labelWithString: key)
        label.font = Swiss.labelFont
        label.textColor = selected ? Swiss.accent : .tertiaryLabelColor

        let text = NSTextField(labelWithString: candidate.text)
        text.font = selected ? Swiss.selectedFont : Swiss.candidateFont
        text.textColor = .labelColor

        let row = NSStackView(views: [bar, label, text])
        row.orientation = .horizontal
        row.alignment = .centerY
        row.spacing = Swiss.grid * 1.5
        row.setCustomSpacing(Swiss.grid * 1.5 - 3, after: bar)
        if let description = candidate.description {
            let note = NSTextField(labelWithString: description)
            note.font = Swiss.smallFont
            note.textColor = .secondaryLabelColor
            row.addArrangedSubview(note)
            row.setCustomSpacing(Swiss.grid * 2, after: text)
        }
        row.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            row.heightAnchor.constraint(equalToConstant: Self.rowHeight),
            bar.widthAnchor.constraint(equalToConstant: 3),
            bar.heightAnchor.constraint(equalTo: row.heightAnchor),
            label.widthAnchor.constraint(equalToConstant: Swiss.grid),
        ])
        stackView.addArrangedSubview(row)
        rowViews.append(row)
    }

    private func addFooter(_ text: String) {
        let rule = NSBox()
        rule.boxType = .custom
        rule.borderWidth = 0
        rule.fillColor = Swiss.rule
        rule.translatesAutoresizingMaskIntoConstraints = false

        let label = NSTextField(labelWithString: text)
        label.font = Swiss.smallFont
        label.textColor = .secondaryLabelColor

        let footer = NSStackView(views: [rule, label])
        footer.orientation = .vertical
        footer.alignment = .leading
        footer.spacing = Swiss.grid
        footer.edgeInsets = NSEdgeInsets(top: Swiss.grid, left: Swiss.grid * 2, bottom: 0, right: 0)
        footer.translatesAutoresizingMaskIntoConstraints = false
        stackView.addArrangedSubview(footer)
        NSLayoutConstraint.activate([
            rule.heightAnchor.constraint(equalToConstant: 1),
            rule.widthAnchor.constraint(equalTo: footer.widthAnchor, constant: -Swiss.grid * 2),
            footer.widthAnchor.constraint(equalTo: stackView.widthAnchor, constant: -Swiss.grid * 2),
        ])
        rowViews.append(footer)
    }

    private var lastCursorRect: NSRect = .zero

    private func positionPanel(cursorRect: NSRect?) {
        if let rect = cursorRect {
            lastCursorRect = rect
        }
        let cursorRect = lastCursorRect

        stackView.layoutSubtreeIfNeeded()
        let contentSize = stackView.fittingSize
        let panelWidth = max(contentSize.width, Self.minPanelWidth)
        let panelHeight = contentSize.height

        guard cursorRect != .zero else {
            panel.setFrame(
                NSRect(x: 100, y: 100, width: panelWidth, height: panelHeight), display: true)
            panel.orderFront(nil)
            return
        }

        // Flip above the cursor when the panel would fall off the bottom of
        // the screen.
        let showAbove: Bool
        if let screen = NSScreen.main {
            showAbove = cursorRect.origin.y - panelHeight < screen.visibleFrame.origin.y
        } else {
            showAbove = false
        }

        let gap: CGFloat = 4
        let originY: CGFloat
        if showAbove {
            originY = cursorRect.origin.y + cursorRect.size.height + gap
        } else {
            originY = cursorRect.origin.y - panelHeight - gap
        }

        panel.setFrame(
            NSRect(x: cursorRect.origin.x, y: originY, width: panelWidth, height: panelHeight),
            display: true)
        panel.orderFront(nil)
    }
}

/// Square badge near the cursor naming the input mode (`あ` / `A`), shown
/// briefly when the mode changes.
class ModeBadgeController {
    private static let side = Swiss.grid * 4
    private static let duration: TimeInterval = 0.6

    private let panel = Swiss.panel()
    private let label = NSTextField(labelWithString: "")
    private var hideWork: DispatchWorkItem?

    init() {
        let box = Swiss.box(fill: .labelColor)
        box.borderWidth = 0
        panel.contentView = box
        label.font = NSFont.systemFont(ofSize: 15, weight: .semibold)
        label.textColor = .textBackgroundColor
        label.alignment = .center
        label.translatesAutoresizingMaskIntoConstraints = false
        let content = box.contentView!
        content.addSubview(label)
        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: content.centerXAnchor),
            label.centerYAnchor.constraint(equalTo: content.centerYAnchor),
        ])
    }

    func show(mode: String, near cursorRect: NSRect) {
        label.stringValue = mode == "ascii" ? "A" : "あ"
        let origin =
            cursorRect == .zero
            ? NSPoint(x: 100, y: 100)
            : NSPoint(x: cursorRect.minX, y: cursorRect.minY - Self.side - 4)
        panel.setFrame(
            NSRect(origin: origin, size: NSSize(width: Self.side, height: Self.side)),
            display: true)
        panel.orderFront(nil)

        hideWork?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.panel.orderOut(nil) }
        hideWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.duration, execute: work)
    }
}

/// The kana under the live conversion, in small type just below the
/// preedit: 会議 in the text, かいぎ under it.
class ReadingPanelController {
    private let panel = Swiss.panel()
    private let label = NSTextField(labelWithString: "")

    init() {
        let box = Swiss.box()
        panel.contentView = box
        label.font = NSFont.systemFont(ofSize: 13)
        label.textColor = .secondaryLabelColor
        label.translatesAutoresizingMaskIntoConstraints = false
        let content = box.contentView!
        content.addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: Swiss.grid),
            label.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -Swiss.grid),
            label.centerYAnchor.constraint(equalTo: content.centerYAnchor),
        ])
    }

    var isVisible: Bool { panel.isVisible }

    /// `cursorRect: nil` keeps the panel where it is (the anchor doesn't
    /// move while a part is being typed).
    func show(_ text: String, cursorRect: NSRect?) {
        label.stringValue = text
        let size = NSSize(width: label.fittingSize.width + Swiss.grid * 2, height: Swiss.grid * 3)
        var origin = panel.frame.origin
        if let rect = cursorRect {
            origin =
                rect == .zero
                ? NSPoint(x: 100, y: 100) : NSPoint(x: rect.minX, y: rect.minY - size.height - 4)
        }
        panel.setFrame(NSRect(origin: origin, size: size), display: true)
        panel.orderFront(nil)
    }

    func hide() {
        panel.orderOut(nil)
    }
}
