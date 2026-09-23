import Foundation

// Swift-side mirror of mimir-im/core/src/server/protocol.rs.
// All positions (caret, attribute start/end) are in Unicode scalar values;
// convert to UTF-16 offsets before passing to IMK APIs.

struct KeyResult: Decodable {
    let consumed: Bool
    let actions: [EngineAction]
    let conversionMs: UInt64?
    let processKeyMs: UInt64?
    /// "kana" or "ascii"; absent from older engines.
    let mode: String?
    /// Text for the panel under an open kanji part (completions or the
    /// reading, per `[display] part_hint`); absent when there is none.
    let hint: String?
    /// Whether a–l pick from the candidate window; absent from older engines.
    let selectKeys: Bool?
    /// `[display] candidate_layout`: "vertical" or "keyboard".
    let candidateLayout: String?
    /// The reading to set above the preedit's last kanji run.
    let ruby: Ruby?
    /// Send `refresh` after this many milliseconds without a key.
    let refreshMs: Int?
}

struct Ruby: Decodable {
    /// Span in the preedit, in Unicode scalar values.
    let start: Int
    let end: Int
    let text: String
}

struct PreeditAttr: Decodable {
    let start: Int
    let end: Int
    let style: String
}

struct CandidateItem: Decodable {
    let text: String
    let description: String?
}

struct InitResult: Decodable {
    let protocolVersion: Int
    let modelName: String
}

enum EngineAction: Decodable {
    case updatePreedit(text: String, caret: Int, attributes: [PreeditAttr])
    case showCandidates(candidates: [CandidateItem], cursor: Int, page: Int, totalPages: Int)
    case hideCandidates
    case commit(text: String)
    case updateAux(text: String)
    case hideAux

    private enum CodingKeys: String, CodingKey {
        case type, text, caret, attributes, candidates, cursor, page, totalPages
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(String.self, forKey: .type)
        switch type {
        case "update_preedit":
            self = .updatePreedit(
                text: try container.decode(String.self, forKey: .text),
                caret: try container.decode(Int.self, forKey: .caret),
                attributes: try container.decodeIfPresent([PreeditAttr].self, forKey: .attributes)
                    ?? []
            )
        case "show_candidates":
            self = .showCandidates(
                candidates: try container.decode([CandidateItem].self, forKey: .candidates),
                cursor: try container.decode(Int.self, forKey: .cursor),
                page: try container.decode(Int.self, forKey: .page),
                totalPages: try container.decode(Int.self, forKey: .totalPages)
            )
        case "hide_candidates":
            self = .hideCandidates
        case "commit":
            self = .commit(text: try container.decode(String.self, forKey: .text))
        case "update_aux":
            self = .updateAux(text: try container.decode(String.self, forKey: .text))
        case "hide_aux":
            self = .hideAux
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .type, in: container, debugDescription: "unknown action type: \(type)")
        }
    }
}

/// JSONDecoder configured for the engine protocol (snake_case keys).
func makeProtocolDecoder() -> JSONDecoder {
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    return decoder
}
