import Cocoa

/// Swiss-style palette and type scale shared by the IME's panels:
/// monochrome, one red accent, flush-left on an 8pt grid, hierarchy by
/// size and weight only, square corners.
enum Swiss {
    static let accent = NSColor(srgbRed: 0.890, green: 0.024, blue: 0.075, alpha: 1)
    static let hairline = NSColor.labelColor.withAlphaComponent(0.15)
    /// 8pt baseline grid.
    static let grid: CGFloat = 8
    static let candidateFont = NSFont.systemFont(ofSize: 17)
    static let selectedFont = NSFont.systemFont(ofSize: 17, weight: .semibold)
    static let keyFont = NSFont(name: "HelveticaNeue-Medium", size: 11)
        ?? NSFont.systemFont(ofSize: 11, weight: .medium)
    static let headFont = NSFont.systemFont(ofSize: 11, weight: .semibold)
    static let smallFont = NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .regular)

    /// A flat square box with a hairline edge, colors resolved per
    /// appearance by AppKit.
    static func box(fill: NSColor = .textBackgroundColor) -> NSBox {
        let box = NSBox()
        box.boxType = .custom
        box.borderColor = hairline
        box.borderWidth = 1
        box.cornerRadius = 0
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

/// The candidate list, drawn directly so every element sits on the grid:
///
///     かいぎ                     1 / 3     header: reading, page (right)
///     ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━     2pt rule
///     [a] 会議
///     [s] 懐疑                     [旧]    selected: key in a red square
///
/// Keys are the ones that pick the row (`j k f d u i r e`, see `part.rs`);
/// descriptions share one flush-right column.
///
/// The keyboard layout instead puts each candidate where its key sits, two
/// rows of the two hands' index and middle fingers (descriptions left out):
///
///     かいぎ                                  1 / 3
///     ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
///     [e] 快技   [r] 回議      [u] 会ぎ   [i] 海技
///     [d] 懐疑   [f] 會議      [j] 会議   [k] かいぎ
final class CandidateListView: NSView {
    struct Row {
        let key: String
        let text: String
        let note: String?
        let selected: Bool
    }

    var header: String?
    var page: String?
    var rows: [Row] = []
    /// Draw the keyboard layout instead of the list.
    var keyboard = false

    /// Keyboard layout: (row, column) of each page position, in the order
    /// of the select keys j k f d u i r e.
    private static let slots = [(1, 2), (1, 3), (1, 1), (1, 0), (0, 2), (0, 3), (0, 1), (0, 0)]
    /// The gap between the hands, on top of a cell's own.
    private let handGap = Swiss.grid * 2
    private var cellWidth: CGFloat {
        let textW = rows.map { size($0.text, Swiss.selectedFont).width }.max() ?? 0
        return keySide + Swiss.grid * 1.5 + textW + Swiss.grid * 2
    }

    private let pad = Swiss.grid * 2
    private let rowHeight = Swiss.grid * 4
    private let keySide = Swiss.grid * 2
    private var textX: CGFloat { pad + keySide + Swiss.grid * 1.5 }
    private var hasHeader: Bool { header != nil || page != nil }
    private var headerHeight: CGFloat { hasHeader ? Swiss.grid * 3 + 2 : 0 }

    override var isFlipped: Bool { true }

    private func size(_ text: String, _ font: NSFont) -> NSSize {
        (text as NSString).size(withAttributes: [.font: font])
    }

    func fittingContentSize() -> NSSize {
        if keyboard {
            let headW =
                pad * 2 + size(header ?? "", Swiss.headFont).width
                + size(page ?? "", Swiss.smallFont).width + Swiss.grid * 3
            let width = max(pad * 2 + cellWidth * 4 + handGap, headW)
            let height = Swiss.grid + headerHeight + rowHeight * 2 + Swiss.grid
            return NSSize(width: ceil(width), height: height)
        }
        let textW = rows.map { size($0.text, Swiss.selectedFont).width }.max() ?? 0
        let noteW = rows.compactMap { $0.note.map { size($0, Swiss.smallFont).width } }.max()
        var width = textX + textW + pad
        if let noteW { width += Swiss.grid * 3 + noteW }
        let headW =
            pad * 2 + size(header ?? "", Swiss.headFont).width
            + size(page ?? "", Swiss.smallFont).width + Swiss.grid * 3
        width = max(width, headW, Swiss.grid * 24)
        let height = Swiss.grid + headerHeight + CGFloat(rows.count) * rowHeight + Swiss.grid
        return NSSize(width: ceil(width), height: height)
    }

    override func draw(_ dirtyRect: NSRect) {
        var y = Swiss.grid
        if hasHeader {
            let head = header ?? ""
            (head as NSString).draw(
                at: NSPoint(x: pad, y: y),
                withAttributes: [.font: Swiss.headFont, .foregroundColor: NSColor.labelColor])
            if let page {
                let w = size(page, Swiss.smallFont).width
                (page as NSString).draw(
                    at: NSPoint(x: bounds.width - pad - w, y: y + 1),
                    withAttributes: [
                        .font: Swiss.smallFont, .foregroundColor: NSColor.secondaryLabelColor,
                    ])
            }
            y += Swiss.grid * 3
            NSColor.labelColor.setFill()
            NSRect(x: pad, y: y - 4, width: bounds.width - pad * 2, height: 2).fill()
            y += 2
        }
        if keyboard {
            for (index, row) in rows.enumerated() where index < Self.slots.count {
                let (line, column) = Self.slots[index]
                let x = pad + CGFloat(column) * cellWidth + (column >= 2 ? handGap : 0)
                drawCell(row, x: x, mid: y + CGFloat(line) * rowHeight + rowHeight / 2)
            }
            return
        }
        for row in rows {
            let mid = y + rowHeight / 2
            drawCell(row, x: pad, mid: mid)
            if let note = row.note {
                let noteSize = size(note, Swiss.smallFont)
                (note as NSString).draw(
                    at: NSPoint(x: bounds.width - pad - noteSize.width, y: mid - noteSize.height / 2),
                    withAttributes: [
                        .font: Swiss.smallFont, .foregroundColor: NSColor.secondaryLabelColor,
                    ])
            }
            y += rowHeight
        }
    }

    /// A key square (red when selected) and the candidate beside it.
    private func drawCell(_ row: Row, x: CGFloat, mid: CGFloat) {
        do {
            let square = NSRect(x: x, y: mid - keySide / 2, width: keySide, height: keySide)
            if row.selected {
                Swiss.accent.setFill()
                square.fill()
            }
            let keySize = size(row.key, Swiss.keyFont)
            (row.key as NSString).draw(
                at: NSPoint(x: square.midX - keySize.width / 2, y: square.midY - keySize.height / 2),
                withAttributes: [
                    .font: Swiss.keyFont,
                    .foregroundColor: row.selected ? NSColor.white : NSColor.tertiaryLabelColor,
                ])
            let font = row.selected ? Swiss.selectedFont : Swiss.candidateFont
            let textH = size(row.text, font).height
            (row.text as NSString).draw(
                at: NSPoint(x: x + textX - pad, y: mid - textH / 2),
                withAttributes: [.font: font, .foregroundColor: NSColor.labelColor])
        }
    }
}

/// Custom candidate window (borderless non-activating NSPanel).
///
/// The engine pre-paginates: `show` receives only the visible page plus
/// page metadata, so this controller only maps it onto `CandidateListView`.
/// The aux text (the reading, see `part.rs`) heads the list.
class CandidateWindowController {
    static let selectKeys = Array("jkfduire")

    private let panel: NSPanel
    private let listView = CandidateListView()
    private var auxText: String?

    private struct PageState {
        let candidates: [CandidateItem]
        let cursor: Int
        let page: Int
        let totalPages: Int
    }
    private var pageState: PageState?
    /// Label the rows with the select keys (off once the selection was
    /// stepped with Ctrl+N/P, when the keys type on instead).
    var showsKeys = true
    /// `[display] candidate_layout`, from the engine.
    var layout = "vertical"

    init() {
        panel = Swiss.panel()
        let box = Swiss.box()
        panel.contentView = box
        listView.translatesAutoresizingMaskIntoConstraints = false
        let content = box.contentView!
        content.addSubview(listView)
        NSLayoutConstraint.activate([
            listView.topAnchor.constraint(equalTo: content.topAnchor),
            listView.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            listView.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            listView.bottomAnchor.constraint(equalTo: content.bottomAnchor),
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

    /// Update the aux header; re-renders in place if the window is visible.
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
        guard let state = pageState else {
            hide()
            return
        }
        // An empty list still renders while the aux has text: a
        // source-filtered view narrowed to an empty source must not vanish.
        if state.candidates.isEmpty && (auxText ?? "").isEmpty {
            hide()
            return
        }
        listView.header = auxText.flatMap { $0.isEmpty ? nil : $0 }
        listView.keyboard = layout == "keyboard"
        listView.page = state.totalPages > 1 ? "\(state.page + 1) / \(state.totalPages)" : nil
        listView.rows = state.candidates.enumerated().map { index, candidate in
            CandidateListView.Row(
                key: showsKeys && index < Self.selectKeys.count ? String(Self.selectKeys[index]) : "",
                text: candidate.text,
                note: candidate.description,
                selected: index == state.cursor)
        }
        listView.needsDisplay = true
        positionPanel(cursorRect: cursorRect, size: listView.fittingContentSize())
    }

    private var lastCursorRect: NSRect = .zero

    private func positionPanel(cursorRect: NSRect?, size: NSSize) {
        if let rect = cursorRect {
            lastCursorRect = rect
        }
        let cursorRect = lastCursorRect

        guard cursorRect != .zero else {
            panel.setFrame(NSRect(origin: NSPoint(x: 100, y: 100), size: size), display: true)
            panel.orderFront(nil)
            return
        }

        // Flip above the cursor when the panel would fall off the bottom of
        // the screen.
        let showAbove: Bool
        if let screen = NSScreen.main {
            showAbove = cursorRect.origin.y - size.height < screen.visibleFrame.origin.y
        } else {
            showAbove = false
        }

        let gap: CGFloat = 4
        let originY =
            showAbove
            ? cursorRect.maxY + gap
            : cursorRect.origin.y - size.height - gap
        panel.setFrame(
            NSRect(origin: NSPoint(x: cursorRect.origin.x, y: originY), size: size), display: true)
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

/// The hint under an open kanji part (`[display] part_hint`): the words Tab
/// would complete to, or the kana under the live conversion. One line of
/// plain text on the grid — no rule, no label; it only accompanies the
/// preedit.
final class ReadingView: NSView {
    static let font = NSFont.systemFont(ofSize: 13)
    var text = "" {
        didSet { needsDisplay = true }
    }
    private let pad = Swiss.grid

    override var isFlipped: Bool { true }

    func fittingContentSize() -> NSSize {
        let w = (text as NSString).size(withAttributes: [.font: Self.font]).width
        return NSSize(width: ceil(w) + pad * 2, height: Swiss.grid * 3)
    }

    override func draw(_ dirtyRect: NSRect) {
        let h = (text as NSString).size(withAttributes: [.font: Self.font]).height
        (text as NSString).draw(
            at: NSPoint(x: pad, y: (bounds.height - h) / 2),
            withAttributes: [.font: Self.font, .foregroundColor: NSColor.labelColor])
    }
}

class ReadingPanelController {
    private let panel = Swiss.panel()
    private let view = ReadingView()

    init() {
        let box = Swiss.box()
        panel.contentView = box
        view.translatesAutoresizingMaskIntoConstraints = false
        let content = box.contentView!
        content.addSubview(view)
        NSLayoutConstraint.activate([
            view.topAnchor.constraint(equalTo: content.topAnchor),
            view.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            view.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            view.bottomAnchor.constraint(equalTo: content.bottomAnchor),
        ])
    }

    var isVisible: Bool { panel.isVisible }

    /// `cursorRect: nil` keeps the panel where it is (the anchor doesn't
    /// move while a part is being typed).
    func show(_ text: String, cursorRect: NSRect?) {
        view.text = text
        let size = view.fittingContentSize()
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
