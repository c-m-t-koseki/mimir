//! Stdio JSON-RPC server wrapping [`InputMethodEngine`].
//!
//! This is the platform-independent core of `mimir-imserver`, the engine
//! process spawned by the macOS (Swift/InputMethodKit) frontend. See
//! [`protocol`] for the wire format.

pub mod protocol;

use serde_json::{Value, json};

use crate::config::Settings;
use crate::config::settings::CandidateLayout;
use crate::core::candidate::CandidateList;
use crate::core::engine::{EngineAction, EngineConfig, EngineResult, InputMethodEngine};
use crate::core::keycode::{KeyEvent, Keysym};
use crate::core::keymap::Keymap;
use crate::core::part::{PartLayer, PartMode};
use crate::core::state::InputState;

use protocol::{
    Action, CandidateItem, InitResult, KeyResult, LearningEntryParams, PROTOCOL_VERSION,
    PreeditAttr, ProcessKeyParams, Request, Response, RpcError, Ruby, SelectCandidateParams,
    SetConfigParams, StatusResult, SurroundingTextParams,
};

/// JSON-RPC dispatcher owning the engine instance.
///
/// One server serves one frontend over one stdin/stdout pair; engine state
/// (Empty → Composing → Conversion) lives here across requests.
pub struct ImServer {
    engine: InputMethodEngine,
    /// Kanji-part layer every key goes through (plain kana / part / ASCII).
    part: PartLayer,
    /// Reported with every key result, for the frontend's candidate window.
    candidate_layout: CandidateLayout,
    /// Pending settings, consumed by the first successful `init`.
    settings: Option<Settings>,
    initialized: bool,
}

impl Default for ImServer {
    fn default() -> Self {
        Self::new()
    }
}

impl ImServer {
    /// Create a server with settings loaded from config.toml (or defaults).
    pub fn new() -> Self {
        let settings = Settings::load_or_default();
        Self::with_settings(settings)
    }

    pub fn with_settings(settings: Settings) -> Self {
        let config = EngineConfig::from_settings(&settings);
        let mut engine = InputMethodEngine::with_config(config);
        let mut part = part_layer(&settings);
        part.attach(&mut engine);
        Self {
            engine,
            part,
            candidate_layout: settings.display.candidate_layout,
            settings: Some(settings),
            initialized: false,
        }
    }

    /// Save the learning cache (called on EOF/shutdown).
    pub fn save_learning(&mut self) {
        self.engine.save_learning();
    }

    /// Handle one request line. Returns the response line to write, or
    /// `None` for notifications (requests without an id).
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let request: Request = match serde_json::from_str(line) {
            Ok(req) => req,
            Err(e) => {
                let resp = Response::failure(
                    Value::Null,
                    RpcError::new(RpcError::PARSE_ERROR, format!("parse error: {e}")),
                );
                return Some(serde_json::to_string(&resp).expect("response serialization"));
            }
        };

        let id = request.id.clone();
        let result = self.dispatch(&request.method, request.params);

        let id = id?; // notification: execute but don't respond
        let resp = match result {
            Ok(value) => Response::success(id, value),
            Err(error) => Response::failure(id, error),
        };
        Some(serde_json::to_string(&resp).expect("response serialization"))
    }

    fn dispatch(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            "init" => self.handle_init(),
            "process_key" => {
                let params: ProcessKeyParams = parse_params(params)?;
                let event =
                    KeyEvent::new(Keysym(params.keysym), params.modifiers, !params.is_release);
                let result = self.part.process_key(&mut self.engine, &event);
                self.key_result(result)
            }
            "select_candidate" => {
                let params: SelectCandidateParams = parse_params(params)?;
                if params.page_index >= CandidateList::DEFAULT_PAGE_SIZE {
                    return Err(RpcError::new(
                        RpcError::INVALID_PARAMS,
                        format!("page_index out of range: {}", params.page_index),
                    ));
                }
                let result = self.engine.select_candidate_on_page(params.page_index);
                self.key_result(result)
            }
            "commit" => {
                let result = self.part.flush(&mut self.engine);
                self.key_result(result)
            }
            "refresh" => {
                let result = self.part.refresh(&self.engine);
                self.key_result(result)
            }
            "reset" => {
                self.engine.reset();
                self.part.attach(&mut self.engine);
                Ok(json!({}))
            }
            "set_surrounding_text" => {
                let params: SurroundingTextParams = parse_params(params)?;
                self.engine
                    .set_surrounding_text_at(&params.text, params.cursor_pos);
                Ok(json!({}))
            }
            "get_config" => {
                let settings = Settings::load_or_default();
                Ok(json!({
                    "part_hint": settings.display.part_hint,
                    "candidate_layout": settings.display.candidate_layout,
                    "candidate_window": settings.display.candidate_window,
                    "reading_display": settings.display.reading_display,
                    "tail_delay_ms": settings.display.tail_delay_ms,
                    "tail_delete_delay_ms": settings.display.tail_delete_delay_ms,
                    "keys": settings.keys,
                    "predictive": settings.conversion.predictive,
                    "skk_style": settings.conversion.skk_style,
                    "model": settings.conversion.model,
                    "light_model": settings.conversion.light_model,
                    "learning": settings.learning.enabled,
                    "date": settings.date.enabled,
                    "models": settings.models.keys().collect::<Vec<_>>(),
                    "profiles": settings.profiles,
                }))
            }
            "set_config" => {
                let params: SetConfigParams = parse_params(params)?;
                let value = match params.value {
                    Value::Null => None,
                    Value::Bool(b) => Some(toml::Value::Boolean(b)),
                    Value::String(s) => Some(toml::Value::String(s)),
                    Value::Number(n) if n.is_i64() => {
                        Some(toml::Value::Integer(n.as_i64().unwrap_or(0)))
                    }
                    other => {
                        return Err(RpcError::new(
                            RpcError::INVALID_PARAMS,
                            format!("unsupported value: {other}"),
                        ));
                    }
                };
                Settings::set_value(&params.section, &params.key, value)
                    .map_err(|e| RpcError::new(RpcError::INVALID_PARAMS, format!("{e:#}")))?;
                if !applies_live(&params.section, &params.key) {
                    return Ok(json!({"restart": true}));
                }
                let settings = Settings::load_or_default();
                self.part.flush(&mut self.engine);
                self.part = part_layer(&settings);
                self.part.attach(&mut self.engine);
                self.candidate_layout = settings.display.candidate_layout;
                Ok(json!({"restart": false}))
            }
            "learning_list" => {
                let entries: Vec<_> = self
                    .engine
                    .learning_entries()
                    .into_iter()
                    .map(|(reading, surface, frequency, last_access)| {
                        json!({"reading": reading, "surface": surface,
                               "frequency": frequency, "last_access": last_access})
                    })
                    .collect();
                Ok(json!({ "entries": entries }))
            }
            "learning_remove" => {
                let params: LearningEntryParams = parse_params(params)?;
                let removed = self
                    .engine
                    .learning_remove(&params.reading, &params.surface);
                Ok(json!({ "removed": removed }))
            }
            "learning_clear" => {
                self.engine.learning_clear();
                Ok(json!({}))
            }
            "save_learning" => {
                self.engine.save_learning();
                Ok(json!({}))
            }
            "status" => {
                let state = match self.engine.state() {
                    InputState::Empty => "empty",
                    InputState::Composing { .. } => "composing",
                    InputState::Conversion { .. } => "conversion",
                };
                serde_json::to_value(StatusResult {
                    initialized: self.initialized,
                    model_name: self.engine.model_name(),
                    state,
                })
                .map_err(internal_error)
            }
            other => Err(RpcError::new(
                RpcError::METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            )),
        }
    }

    fn handle_init(&mut self) -> Result<Value, RpcError> {
        if !self.initialized {
            let settings = self
                .settings
                .take()
                .unwrap_or_else(Settings::load_or_default);
            if let Err(e) = self.engine.init_from_settings(&settings) {
                // Keep the settings so a retried `init` uses the same ones.
                self.settings = Some(settings);
                return Err(RpcError::new(RpcError::INIT_FAILED, format!("{e:#}")));
            }
            self.initialized = true;
        }
        serde_json::to_value(InitResult {
            protocol_version: PROTOCOL_VERSION,
            model_name: self.engine.model_name(),
        })
        .map_err(internal_error)
    }

    fn key_result(&self, result: EngineResult) -> Result<Value, RpcError> {
        let actions = result.actions.into_iter().map(to_action).collect();
        serde_json::to_value(KeyResult {
            consumed: result.consumed,
            actions,
            conversion_ms: self.engine.last_conversion_ms(),
            process_key_ms: self.engine.last_process_key_ms(),
            mode: match self.part.mode() {
                PartMode::Ascii => "ascii",
                PartMode::Plain | PartMode::Part => "kana",
            },
            hint: self.part.hint(&self.engine),
            select_keys: self.part.picks_by_letter(),
            select_chars: self.part.select_keys(),
            candidate_layout: self.candidate_layout,
            ruby: self
                .part
                .ruby(&self.engine)
                .map(|(start, end, text)| Ruby { start, end, text }),
            refresh_ms: self.part.refresh_after_ms(&self.engine),
        })
        .map_err(internal_error)
    }
}

/// The part layer as `settings` configure it.
fn part_layer(settings: &Settings) -> PartLayer {
    PartLayer::with_hint(settings.display.part_hint)
        .with_skk_style(settings.conversion.skk_style)
        .with_keys(Keymap::parse(&settings.keys).unwrap_or_default())
        .with_reading_display(
            settings.display.reading_display,
            settings.display.tail_chars,
            settings.display.tail_delay_ms,
            settings.display.tail_delete_delay_ms,
        )
}

/// Settings only the part layer and the key results read: `set_config`
/// applies them at once, where anything else waits for an engine restart
/// (models and dictionaries load at `init`).
fn applies_live(section: &str, key: &str) -> bool {
    match section {
        "keys" => true,
        "display" => matches!(
            key,
            "part_hint"
                | "reading_display"
                | "tail_chars"
                | "tail_delay_ms"
                | "tail_delete_delay_ms"
                | "candidate_layout"
        ),
        "conversion" => key == "skk_style",
        _ => false,
    }
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params)
        .map_err(|e| RpcError::new(RpcError::INVALID_PARAMS, format!("invalid params: {e}")))
}

fn internal_error(e: serde_json::Error) -> RpcError {
    RpcError::new(RpcError::INTERNAL_ERROR, format!("internal error: {e}"))
}

fn to_action(action: EngineAction) -> Action {
    use crate::core::preedit::AttributeType;

    match action {
        EngineAction::UpdatePreedit(preedit) => Action::UpdatePreedit {
            caret: preedit.caret(),
            attributes: preedit
                .attributes()
                .iter()
                .map(|a| PreeditAttr {
                    start: a.start,
                    end: a.end,
                    style: match a.attr_type {
                        AttributeType::Underline => "underline",
                        AttributeType::UnderlineDouble => "underline_double",
                        AttributeType::Highlight => "highlight",
                        AttributeType::Reverse => "reverse",
                    },
                })
                .collect(),
            text: preedit.text().to_string(),
        },
        EngineAction::ShowCandidates(list) => Action::ShowCandidates {
            candidates: list
                .page_candidates()
                .iter()
                .map(|c| CandidateItem {
                    text: c.text.clone(),
                    description: c.description.clone().filter(|s| !s.is_empty()),
                })
                .collect(),
            cursor: list.page_cursor(),
            page: list.current_page(),
            total_pages: list.total_pages(),
        },
        EngineAction::HideCandidates => Action::HideCandidates,
        EngineAction::Commit(text) => Action::Commit { text },
        EngineAction::UpdateAuxText(text) => Action::UpdateAux { text },
        EngineAction::HideAuxText => Action::HideAux,
    }
}

#[cfg(test)]
mod tests;
