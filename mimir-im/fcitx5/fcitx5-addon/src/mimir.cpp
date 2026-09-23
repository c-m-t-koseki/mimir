/*
 * Mimir fcitx5 addon implementation
 */

#include "mimir.h"

#include <fcitx-utils/i18n.h>
#include <fcitx-utils/key.h>
#include <fcitx-utils/utf8.h>
#include <fcitx/inputpanel.h>
#include <xkbcommon/xkbcommon-keysyms.h>

namespace fcitx {

// X11 modifier bitmask constants matching the Rust FFI boundary (KeyModifiers::*_MASK).
constexpr uint32_t kShiftMask = 1;    // ShiftMask
constexpr uint32_t kControlMask = 4;  // ControlMask
constexpr uint32_t kAltMask = 8;      // Mod1Mask
constexpr uint32_t kSuperMask = 64;   // Mod4Mask

// --- MimirCandidateWord ---

MimirCandidateWord::MimirCandidateWord(MimirEngine* engine, Text text, int index,
                                           const std::string& description)
    :
#ifndef FCITX5_HAS_CANDIDATE_SET_COMMENT
      // fcitx5 < 5.1.9: append description to candidate text
      CandidateWord([&]() {
          if (!description.empty()) {
              text.append(" " + description);
          }
          return std::move(text);
      }()),
#else
      CandidateWord(std::move(text)),
#endif
      engine_(engine), index_(index) {
#ifdef FCITX5_HAS_CANDIDATE_SET_COMMENT
    // fcitx5 >= 5.1.9: show description as a right-side comment
    if (!description.empty()) {
        setComment(Text(description));
    }
#endif
}

void MimirCandidateWord::select(InputContext* inputContext) const {
    engine_->selectCandidate(inputContext, index_);
}

// --- MimirCandidateList ---

MimirCandidateList::MimirCandidateList(MimirEngine* engine) : engine_(engine) {
    setLayoutHint(CandidateLayoutHint::Vertical);
    setPageSize(9);
    // Row labels only. Selection is Ctrl+digit and is handled by the Rust
    // engine, which consumes bare digits as text input before fcitx5 could
    // act on them.
    setSelectionKey(Key::keyListFromString("1 2 3 4 5 6 7 8 9"));
}

void MimirCandidateList::updateCandidates(::MimirEngine* rustEngine) {
    // Clear existing candidates
    while (totalSize() > 0) {
        remove(0);
    }

    uint32_t count = mimir_engine_get_candidate_count(rustEngine);
    uint32_t cursor = mimir_engine_get_candidate_cursor(rustEngine);

    for (uint32_t i = 0; i < count; i++) {
        const char* text = mimir_engine_get_candidate(rustEngine, i);
        if (text) {
            Text candidateText;
            candidateText.append(std::string(text));
            const char* desc = mimir_engine_get_candidate_description(rustEngine, i);
            std::string description = (desc && desc[0] != '\0') ? std::string(desc) : "";
            append<MimirCandidateWord>(engine_, std::move(candidateText), i, description);
        }
    }

    if (count > 0 && cursor < count) {
        setGlobalCursorIndex(static_cast<int>(cursor));
    }
}

// --- MimirState ---

MimirState::MimirState(MimirEngine* engine, InputContext* ic) : engine_(engine), ic_(ic) {
    // Create Rust engine instance
    rustEngine_ = mimir_engine_new();
}

MimirState::~MimirState() {
    if (rustEngine_) {
        mimir_engine_free(rustEngine_);
    }
}

void MimirState::keyEvent(KeyEvent& keyEvent) {
    if (!rustEngine_) {
        return;
    }

    // Initialize kanji converter on first use (model download + load may take time)
    if (!engineInitialized_) {
        // Show loading message before blocking init
        {
            auto& inputPanel = ic_->inputPanel();
            Text aux;
            aux.append("Mimir: Loading model...");
            inputPanel.setAuxUp(aux);
            ic_->updatePreedit();
            ic_->updateUserInterface(UserInterfaceComponent::InputPanel);
        }

        int initResult = mimir_engine_init(rustEngine_);
        engineInitialized_ = true;

        // Clear loading message
        {
            auto& inputPanel = ic_->inputPanel();
            if (initResult == 0) {
                inputPanel.setAuxUp(Text());
            } else {
                Text aux;
                aux.append("Mimir: Model load failed");
                inputPanel.setAuxUp(aux);
            }
            ic_->updatePreedit();
            ic_->updateUserInterface(UserInterfaceComponent::InputPanel);
        }
    }

    // Convert key event
    uint32_t keysym = keyEvent.key().sym();
    uint32_t state = 0;

    if (keyEvent.key().states().test(KeyState::Shift)) {
        state |= kShiftMask;
    }
    if (keyEvent.key().states().test(KeyState::Ctrl)) {
        state |= kControlMask;
    }
    if (keyEvent.key().states().test(KeyState::Alt)) {
        state |= kAltMask;
    }
    if (keyEvent.key().states().test(KeyState::Super)) {
        state |= kSuperMask;
    }

    int isRelease = keyEvent.isRelease() ? 1 : 0;

    // Capture surrounding text at input start (Empty state) for accurate context.
    // For apps without SurroundingText capability (terminals), this clears
    // the context so stale data doesn't persist.
    if (mimir_engine_is_empty(rustEngine_) && !isRelease) {
        captureSurroundingText();
    }

    // Process key through Rust engine
    int consumed = mimir_engine_process_key(rustEngine_, keysym, state, isRelease);

    if (consumed) {
        keyEvent.filterAndAccept();
    }

    // Always update UI: some not-consumed keys (e.g., Shift toggle) still
    // change engine state and produce UI actions. The has_* flags in the
    // Rust engine guard against unnecessary updates.
    updateUI();
}

void MimirState::reset() {
    if (rustEngine_) {
        mimir_engine_reset(rustEngine_);
    }

    ic_->inputPanel().reset();
    ic_->updatePreedit();
    ic_->updateUserInterface(UserInterfaceComponent::InputPanel);
}

void MimirState::updateUI() {
    if (!rustEngine_) {
        return;
    }

    auto& inputPanel = ic_->inputPanel();

    // On commit: send committed text, then reset the input panel to clear
    // preedit/candidates/aux in one shot.
    // New preedit/candidates/aux are re-set below if the engine produced them.
    if (mimir_engine_has_commit(rustEngine_)) {
        emitPendingCommit();
        inputPanel.reset();
    }

    // Set preedit (new input after commit, or a regular update)
    if (mimir_engine_has_preedit(rustEngine_)) {
        const char* preeditText = mimir_engine_get_preedit(rustEngine_);
        uint32_t preeditLen = mimir_engine_get_preedit_len(rustEngine_);
        uint32_t preeditCaret = mimir_engine_get_preedit_caret(rustEngine_);

        Text preedit;
        if (preeditText && preeditLen > 0) {
            preedit.append(std::string(preeditText, preeditLen), TextFormatFlag::Underline);
            preedit.setCursor(static_cast<int>(preeditCaret));
        }

        if (ic_->capabilityFlags().test(CapabilityFlag::Preedit)) {
            inputPanel.setClientPreedit(preedit);
        } else {
            inputPanel.setPreedit(preedit);
        }
    }

    // Aux text (reading hint shown above candidates)
    if (mimir_engine_has_aux(rustEngine_)) {
        const char* auxText = mimir_engine_get_aux(rustEngine_);
        uint32_t auxLen = mimir_engine_get_aux_len(rustEngine_);

        if (auxText && auxLen > 0) {
            Text aux;
            aux.append(std::string(auxText, auxLen));
            inputPanel.setAuxUp(aux);
        } else {
            inputPanel.setAuxUp(Text());
        }
    }

    // Candidates
    if (mimir_engine_has_candidates(rustEngine_)) {
        if (mimir_engine_should_hide_candidates(rustEngine_)) {
            inputPanel.setCandidateList(nullptr);
        } else {
            auto candidateList = std::make_unique<MimirCandidateList>(engine_);
            candidateList->updateCandidates(rustEngine_);
            inputPanel.setCandidateList(std::move(candidateList));
        }
    }

    ic_->updatePreedit();
    ic_->updateUserInterface(UserInterfaceComponent::InputPanel);
}

void MimirState::captureSurroundingText() {
    if (ic_->capabilityFlags().test(CapabilityFlag::SurroundingText) &&
        ic_->surroundingText().isValid()) {
        const auto& surrounding = ic_->surroundingText();
        const std::string& text = surrounding.text();
        uint32_t cursor = surrounding.cursor();
        mimir_engine_set_surrounding_text(rustEngine_, text.c_str(), cursor);
    } else {
        mimir_engine_set_surrounding_text(rustEngine_, "", 0);
    }
}

void MimirState::emitPendingCommit() {
    const char* commitText = mimir_engine_get_commit(rustEngine_);
    if (commitText && mimir_engine_get_commit_len(rustEngine_) > 0) {
        ic_->commitString(commitText);
    }
}

// --- MimirEngine ---

MimirEngine::MimirEngine(Instance* instance)
    : instance_(instance),
      factory_([this](InputContext& ic) { return new MimirState(this, &ic); }) {
    instance_->inputContextManager().registerProperty("mimirState", &factory_);
}

MimirEngine::~MimirEngine() = default;

void MimirEngine::keyEvent(const InputMethodEntry& entry, KeyEvent& keyEvent) {
    FCITX_UNUSED(entry);

    auto* ic = keyEvent.inputContext();
    auto* state = ic->propertyFor(&factory_);

    state->keyEvent(keyEvent);
}

void MimirEngine::reset(const InputMethodEntry& entry, InputContextEvent& event) {
    FCITX_UNUSED(entry);

    auto* ic = event.inputContext();
    auto* state = ic->propertyFor(&factory_);

    state->reset();
}

void MimirEngine::activate(const InputMethodEntry& entry, InputContextEvent& event) {
    FCITX_UNUSED(entry);

    auto* ic = event.inputContext();
    auto* state = ic->propertyFor(&factory_);

    // Capture surrounding text on activation for accurate context.
    // For apps without SurroundingText capability, this clears the context.
    if (state->rustEngine()) {
        state->captureSurroundingText();
    }
}

void MimirEngine::deactivate(const InputMethodEntry& entry, InputContextEvent& event) {
    FCITX_UNUSED(entry);

    auto* ic = event.inputContext();
    auto* state = ic->propertyFor(&factory_);

    // Commit any pending input on deactivation (mozc-style behavior)
    // This ensures preedit is not lost when Super/Windows key is pressed
    if (state->rustEngine()) {
        if (mimir_engine_commit(state->rustEngine())) {
            state->emitPendingCommit();
        }
        // Persist learning cache on deactivation (azooKey-style)
        mimir_engine_save_learning(state->rustEngine());
    }

    // Invalidate fcitx5's surrounding text and clear Rust-side context
    // so stale data doesn't persist across sessions.
    ic->surroundingText().invalidate();
    if (state->rustEngine()) {
        mimir_engine_set_surrounding_text(state->rustEngine(), "", 0);
    }

    // reset() clears inputPanel (preedit/candidates/aux) and flushes UI
    state->reset();
}

void MimirEngine::selectCandidate(InputContext* ic, int index) {
    auto* state = ic->propertyFor(&factory_);
    auto* rustEngine = state->rustEngine();

    if (!rustEngine) {
        return;
    }

    // Process the selection key (1-9)
    uint32_t keysym = XKB_KEY_1 + index;
    mimir_engine_process_key(rustEngine, keysym, 0, 0);

    state->updateUI();
}

}  // namespace fcitx

// Export the addon factory
FCITX_ADDON_FACTORY(fcitx::MimirEngineFactory);
