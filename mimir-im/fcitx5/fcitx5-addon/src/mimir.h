/*
 * Mimir fcitx5 addon
 */

#ifndef FCITX5_MIMIR_MIMIR_H
#define FCITX5_MIMIR_MIMIR_H

#include <fcitx/addonfactory.h>
#include <fcitx/addonmanager.h>
#include <fcitx/candidatelist.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputmethodengine.h>
#include <fcitx/instance.h>

// Include the Rust FFI header
#include "../../include/mimir.h"

namespace fcitx {

class MimirState;
class MimirEngine;

// Candidate word class
class MimirCandidateWord : public CandidateWord {
public:
    MimirCandidateWord(MimirEngine* engine, Text text, int index,
                         const std::string& description = "");
    void select(InputContext* inputContext) const override;

private:
    MimirEngine* engine_;
    int index_;
};

// Candidate list class
class MimirCandidateList : public CommonCandidateList {
public:
    MimirCandidateList(MimirEngine* engine);
    void updateCandidates(::MimirEngine* rustEngine);

private:
    MimirEngine* engine_;
};

// Per-input-context state
class MimirState : public InputContextProperty {
public:
    MimirState(MimirEngine* engine, InputContext* ic);
    ~MimirState() override;

    void keyEvent(KeyEvent& keyEvent);
    void reset();
    void updateUI();
    void captureSurroundingText();
    void emitPendingCommit();

    ::MimirEngine* rustEngine() { return rustEngine_; }

private:
    MimirEngine* engine_;
    InputContext* ic_;
    ::MimirEngine* rustEngine_{nullptr};
    bool engineInitialized_{false};
};

// Main engine class
class MimirEngine : public InputMethodEngineV3 {
public:
    MimirEngine(Instance* instance);
    ~MimirEngine() override;

    void keyEvent(const InputMethodEntry& entry, KeyEvent& keyEvent) override;
    void reset(const InputMethodEntry& entry, InputContextEvent& event) override;
    void activate(const InputMethodEntry& entry, InputContextEvent& event) override;
    void deactivate(const InputMethodEntry& entry, InputContextEvent& event) override;

    void selectCandidate(InputContext* ic, int index);

private:
    Instance* instance_;
    FactoryFor<MimirState> factory_;
};

class MimirEngineFactory : public AddonFactory {
public:
    AddonInstance* create(AddonManager* manager) override {
        return new MimirEngine(manager->instance());
    }
};

}  // namespace fcitx

#endif  // FCITX5_MIMIR_MIMIR_H
