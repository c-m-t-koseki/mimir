/*
 * Mimir IME - C FFI Header
 *
 * This header defines the C interface to the Mimir IME engine.
 * Use this to integrate Mimir with fcitx5 or other input method frameworks.
 */

#ifndef MIMIR_H
#define MIMIR_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque handle to a Mimir engine instance */
typedef struct MimirEngine MimirEngine;

/*
 * Create a new Mimir engine instance.
 * Returns a pointer to the engine, or NULL on failure.
 * The caller is responsible for freeing the engine with mimir_engine_free().
 */
MimirEngine* mimir_engine_new(void);

/*
 * Initialize the kanji converter (loads the neural network model).
 * This may take a few seconds on first call.
 * Returns 0 on success, -1 on failure.
 */
int mimir_engine_init(MimirEngine* engine);

/*
 * Destroy a Mimir engine instance and free its resources.
 */
void mimir_engine_free(MimirEngine* engine);

/*
 * Process a key event.
 *
 * Parameters:
 *   engine     - The engine instance
 *   keysym     - X11 keysym value
 *   state      - Modifier state (X11 modifier mask)
 *   is_release - 1 if key release, 0 if key press
 *
 * Returns 1 if the key was consumed by the IME, 0 otherwise.
 *
 * After calling this function, check the has_* functions to see what
 * actions need to be taken (update preedit, commit text, etc.)
 */
int mimir_engine_process_key(
    MimirEngine* engine,
    uint32_t keysym,
    uint32_t state,
    int is_release
);

/*
 * Reset the engine state, clearing any pending input.
 */
void mimir_engine_reset(MimirEngine* engine);

/*
 * Set the surrounding text context from the editor.
 * This provides the actual text around the cursor for better conversion accuracy.
 *
 * Parameters:
 *   engine     - The engine instance
 *   text       - The surrounding text (null-terminated UTF-8)
 *   cursor_pos - Cursor position in bytes (UTF-8 byte offset)
 *
 * The text before cursor_pos will be used as left context for conversion.
 */
void mimir_engine_set_surrounding_text(
    MimirEngine* engine,
    const char* text,
    uint32_t cursor_pos
);

/* --- Preedit (composition) text --- */

/*
 * Check if there's a preedit update pending.
 */
int mimir_engine_has_preedit(const MimirEngine* engine);

/*
 * Get the current preedit text.
 * Returns a pointer to a null-terminated UTF-8 string.
 * The pointer is valid until the next process_key call.
 */
const char* mimir_engine_get_preedit(const MimirEngine* engine);

/*
 * Get the preedit text length in bytes (not including null terminator).
 */
uint32_t mimir_engine_get_preedit_len(const MimirEngine* engine);

/*
 * Get the preedit caret (cursor) position in bytes.
 * This indicates where the cursor should be displayed within the preedit text.
 */
uint32_t mimir_engine_get_preedit_caret(const MimirEngine* engine);

/* --- Commit text --- */

/*
 * Check if there's a commit pending.
 */
int mimir_engine_has_commit(const MimirEngine* engine);

/*
 * Get the commit text.
 * Returns a pointer to a null-terminated UTF-8 string.
 * The pointer is valid until the next process_key call.
 */
const char* mimir_engine_get_commit(const MimirEngine* engine);

/*
 * Get the commit text length in bytes (not including null terminator).
 */
uint32_t mimir_engine_get_commit_len(const MimirEngine* engine);

/* --- Candidates --- */

/*
 * Check if there's a candidates update pending.
 */
int mimir_engine_has_candidates(const MimirEngine* engine);

/*
 * Check if candidates should be hidden.
 */
int mimir_engine_should_hide_candidates(const MimirEngine* engine);

/*
 * Get the number of candidates.
 */
uint32_t mimir_engine_get_candidate_count(const MimirEngine* engine);

/*
 * Get a candidate by index.
 * Returns a pointer to a null-terminated UTF-8 string, or NULL if index is out of range.
 * The pointer is valid until the next process_key call.
 */
const char* mimir_engine_get_candidate(const MimirEngine* engine, uint32_t index);

/*
 * Get the per-candidate description (mozc-style right-side comment) by index.
 * Returns a "[…]"-wrapped UTF-8 string ready for fcitx5 setComment
 * (e.g. "[三点リーダ]", "[全]英大文字"), or an empty string when the
 * candidate has no description. Source labels ("🤖 AI", "📚 辞書", ...)
 * are surfaced via the aux text instead.
 * Returns NULL if index is out of range.
 * The pointer is valid until the next process_key call.
 */
const char* mimir_engine_get_candidate_description(const MimirEngine* engine, uint32_t index);

/*
 * Get the current candidate cursor position (selected index).
 */
uint32_t mimir_engine_get_candidate_cursor(const MimirEngine* engine);

/* --- Auxiliary text (reading hint) --- */

/*
 * Check if there's an aux text update pending.
 */
int mimir_engine_has_aux(const MimirEngine* engine);

/*
 * Get the aux text.
 * Returns a pointer to a null-terminated UTF-8 string.
 * The pointer is valid until the next process_key call.
 */
const char* mimir_engine_get_aux(const MimirEngine* engine);

/*
 * Get the aux text length in bytes (not including null terminator).
 */
uint32_t mimir_engine_get_aux_len(const MimirEngine* engine);

/* --- Timing --- */

/*
 * Get the last conversion time in milliseconds (inference only).
 */
uint64_t mimir_engine_get_last_conversion_ms(const MimirEngine* engine);

/*
 * Get the last process_key time in milliseconds (input to result, end-to-end).
 */
uint64_t mimir_engine_get_last_process_key_ms(const MimirEngine* engine);

/* --- Learning cache --- */

/*
 * Save the learning cache to disk if there are unsaved changes.
 * Called on deactivate (IME switch / window switch) for periodic persistence.
 */
void mimir_engine_save_learning(MimirEngine* engine);

/* --- State query --- */

/*
 * Check if the engine is in the Empty (idle) state.
 * Returns 1 if empty, 0 if composing or converting.
 */
int mimir_engine_is_empty(const MimirEngine* engine);

/* --- Focus handling --- */

/*
 * Commit any pending input.
 * This is used when the IME is deactivated (focus lost) to commit preedit.
 * Returns 1 if text was committed, 0 otherwise.
 * After this call, check mimir_engine_has_commit() and get the text
 * with mimir_engine_get_commit().
 */
int mimir_engine_commit(MimirEngine* engine);

#ifdef __cplusplus
}
#endif

#endif /* MIMIR_H */
