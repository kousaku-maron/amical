#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;
use chrono::{DateTime, Datelike, Local, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
  collections::{HashMap, HashSet},
  env,
  fs,
  path::{Path, PathBuf},
  process::Command,
  sync::Mutex,
  time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{
  image::Image,
  menu::{MenuBuilder, MenuEvent, MenuItem},
  tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
  AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder,
};
use reqwest::blocking::{
  Client,
  multipart::{Form, Part},
};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;
use yrs::{
  updates::decoder::Decode, Doc, ReadTxn, StateVector, Text, Transact, Update,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NoteIdPayload {
  note_id: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct YjsUpdatePayload {
  note_id: i64,
  update: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioChunkPayload {
  chunk: Vec<f32>,
  is_final_chunk: bool,
}

#[tauri::command]
fn notes_replace_yjs_updates(
  payload: YjsUpdatePayload,
  state: State<AppState>,
) -> Result<(), String> {
  let mut conn = state
    .db
    .lock()
    .map_err(|_| "Failed to lock database".to_string())?;
  let now = now_unix_seconds();
  let tx = conn
    .transaction()
    .map_err(|error| error.to_string())?;
  tx.execute(
    "DELETE FROM yjs_updates WHERE note_id = ?1",
    params![payload.note_id],
  )
  .map_err(|error| error.to_string())?;
  tx.execute(
    "INSERT INTO yjs_updates (note_id, update_data, created_at) VALUES (?1, ?2, ?3)",
    params![payload.note_id, payload.update, now],
  )
  .map_err(|error| error.to_string())?;
  tx.commit().map_err(|error| error.to_string())?;
  Ok(())
}

#[tauri::command]
fn notes_save_yjs_update(
  payload: YjsUpdatePayload,
  state: State<AppState>,
) -> Result<(), String> {
  let conn = state
    .db
    .lock()
    .map_err(|_| "Failed to lock database".to_string())?;
  let now = now_unix_seconds();
  conn
    .execute(
      "INSERT INTO yjs_updates (note_id, update_data, created_at) VALUES (?1, ?2, ?3)",
      params![payload.note_id, payload.update, now],
    )
    .map_err(|error| error.to_string())?;
  Ok(())
}

#[tauri::command]
fn notes_load_yjs_updates(
  payload: NoteIdPayload,
  state: State<AppState>,
) -> Result<Vec<Vec<u8>>, String> {
  let conn = state
    .db
    .lock()
    .map_err(|_| "Failed to lock database".to_string())?;
  let mut stmt = conn
    .prepare("SELECT update_data FROM yjs_updates WHERE note_id = ?1 ORDER BY id ASC")
    .map_err(|error| error.to_string())?;
  let rows = stmt
    .query_map(params![payload.note_id], |row| row.get::<_, Vec<u8>>(0))
    .map_err(|error| error.to_string())?;

  let mut updates = Vec::new();
  for row in rows {
    updates.push(row.map_err(|error| error.to_string())?);
  }
  Ok(updates)
}

#[tauri::command]
fn audio_data_chunk(payload: AudioChunkPayload, state: State<AppState>) -> Result<(), String> {
  let mut recording = state
    .recording
    .lock()
    .map_err(|_| "Failed to lock recording state".to_string())?;
  if recording.state != "recording" {
    return Ok(());
  }
  if !payload.chunk.is_empty() {
    recording.audio_samples.extend(payload.chunk);
  }
  if payload.is_final_chunk {
    // Mark end of stream; no-op for now.
  }
  Ok(())
}

#[tauri::command]
fn trpc(
  path: String,
  r#type: String,
  input: Value,
  app: tauri::AppHandle,
  state: State<AppState>,
) -> Result<Value, String> {
  let op_type = r#type;
  let input_json = input.get("json").cloned().unwrap_or(Value::Null);
  let machine_id = state.machine_id.clone();
  let app_data_dir = state.app_data_dir.clone();
  let mut settings_guard = state
    .settings
    .lock()
    .map_err(|_| "Failed to lock settings state".to_string())?;
  let payload = trpc_dispatch(
    &path,
    &op_type,
    &input_json,
    &mut settings_guard,
    &machine_id,
    &state.db,
    &app_data_dir,
    &state.recording,
    &app,
  )?;

  if op_type == "mutation" && path != "settings.resetApp" {
    let snapshot = settings_guard.clone();
    drop(settings_guard);
    if let Err(error) = persist_settings(&state.settings_path, &snapshot) {
      eprintln!("Failed to persist settings: {error}");
    }
  }

  Ok(json!({ "json": payload, "meta": Value::Null }))
}

fn trpc_dispatch(
  path: &str,
  op_type: &str,
  input: &Value,
  settings: &mut SettingsState,
  machine_id: &str,
  db: &Mutex<Connection>,
  app_data_dir: &PathBuf,
  recording: &Mutex<RecordingSession>,
  app: &tauri::AppHandle,
) -> Result<Value, String> {
  Ok(match (path, op_type) {
    ("settings.getTelemetryConfig", "query") => json!({
      "apiKey": "",
      "host": "",
      "machineId": machine_id,
      "enabled": settings.telemetry.enabled,
      "feedbackSurveyId": ""
    }),
    ("settings.getTelemetrySettings", "query") => json!({ "enabled": settings.telemetry.enabled }),
    ("settings.updateTelemetrySettings", "mutation") => {
      if let Some(enabled) = input.get("enabled").and_then(|v| v.as_bool()) {
        settings.telemetry.enabled = enabled;
      }
      json!({ "success": true })
    }
    ("settings.getShortcuts", "query") => json!({
      "pushToTalk": settings.shortcuts.push_to_talk,
      "toggleRecording": settings.shortcuts.toggle_recording
    }),
    ("settings.setShortcut", "mutation") => {
      if let Some(kind) = input.get("type").and_then(|v| v.as_str()) {
        let shortcut = input
          .get("shortcut")
          .and_then(|v| v.as_array())
          .map(to_string_vec)
          .unwrap_or_default();
        match kind {
          "pushToTalk" => settings.shortcuts.push_to_talk = shortcut,
          "toggleRecording" => settings.shortcuts.toggle_recording = shortcut,
          _ => {}
        }
      }
      json!({ "success": true })
    }
    ("settings.setShortcutRecordingState", "mutation") => json!(true),
    ("settings.getDictationSettings", "query") => json!({
      "autoDetectEnabled": settings.dictation.auto_detect_enabled,
      "selectedLanguage": settings.dictation.selected_language
    }),
    ("settings.setDictationSettings", "mutation") => {
      if let Some(enabled) = input
        .get("autoDetectEnabled")
        .and_then(|v| v.as_bool())
      {
        settings.dictation.auto_detect_enabled = enabled;
      }
      if let Some(language) = input
        .get("selectedLanguage")
        .and_then(|v| v.as_str())
      {
        settings.dictation.selected_language = language.to_string();
      }
      json!(true)
    }
    ("settings.getFormatterConfig", "query") => json!(settings.formatter_config),
    ("settings.setFormatterConfig", "mutation") => {
      if let Some(enabled) = input.get("enabled").and_then(|v| v.as_bool()) {
        settings.formatter_config.enabled = enabled;
      }
      if let Some(model_id) = input.get("modelId") {
        settings.formatter_config.model_id = match model_id {
          Value::String(value) => Some(value.to_string()),
          Value::Null => None,
          _ => settings.formatter_config.model_id.clone(),
        };
      }
      if let Some(fallback_id) = input.get("fallbackModelId") {
        settings.formatter_config.fallback_model_id = match fallback_id {
          Value::String(value) => Some(value.to_string()),
          Value::Null => None,
          _ => settings.formatter_config.fallback_model_id.clone(),
        };
      }
      json!(true)
    }
    ("settings.getPreferences", "query") => json!({
      "launchAtLogin": settings.preferences.launch_at_login,
      "minimizeToTray": settings.preferences.minimize_to_tray,
      "showWidgetWhileInactive": settings.preferences.show_widget_while_inactive,
      "showInDock": settings.preferences.show_in_dock
    }),
    ("settings.updatePreferences", "mutation") => {
      if let Some(value) = input.get("launchAtLogin").and_then(|v| v.as_bool()) {
        settings.preferences.launch_at_login = value;
      }
      if let Some(value) = input.get("minimizeToTray").and_then(|v| v.as_bool()) {
        settings.preferences.minimize_to_tray = value;
      }
      if let Some(value) = input.get("showWidgetWhileInactive").and_then(|v| v.as_bool()) {
        settings.preferences.show_widget_while_inactive = value;
        if let Some(widget) = app.get_webview_window("widget") {
          if value {
            let _ = widget.show();
          } else {
            let _ = widget.hide();
          }
        }
      }
      if let Some(value) = input.get("showInDock").and_then(|v| v.as_bool()) {
        settings.preferences.show_in_dock = value;
      }
      json!(true)
    }
    ("settings.setPreferredMicrophone", "mutation") => {
      let device_name = input.get("deviceName").and_then(|v| v.as_str());
      settings.recording.preferred_microphone_name = device_name.map(|v| v.to_string());
      json!(true)
    }
    ("settings.updateTranscriptionSettings", "mutation") => {
      if let Some(value) = input.get("preloadWhisperModel").and_then(|v| v.as_bool()) {
        settings.transcription.preload_whisper_model = value;
      }
      json!(true)
    }
    ("settings.updateUITheme", "mutation") => {
      if let Some(theme) = input.get("theme").and_then(|v| v.as_str()) {
        settings.ui_theme = theme.to_string();
      }
      json!(true)
    }
    ("settings.getSettings", "query") => json!({
      "recording": {
        "preferredMicrophoneName": settings.recording.preferred_microphone_name
      },
      "transcription": {
        "preloadWhisperModel": settings.transcription.preload_whisper_model
      },
      "ui": {
        "theme": settings.ui_theme
      }
    }),
    ("settings.getDataPath", "query") => {
      json!(app_data_dir.to_string_lossy())
    }
    ("settings.getLogFilePath", "query") => {
      json!(log_file_path(app_data_dir).to_string_lossy())
    }
    ("settings.getMachineId", "query") => json!(machine_id),
    ("settings.getAppVersion", "query") => json!(env!("CARGO_PKG_VERSION")),
    ("settings.getModelProvidersConfig", "query") => {
      Value::Object(settings.model_providers_config.clone())
    }
    ("settings.getTranscriptionProvidersConfig", "query") => {
      Value::Object(settings.transcription_providers_config.clone())
    }
    ("settings.setOpenRouterConfig", "mutation") => {
      upsert_config(&mut settings.model_providers_config, "openRouter", input);
      json!(true)
    }
    ("settings.setOllamaConfig", "mutation") => {
      upsert_config(&mut settings.model_providers_config, "ollama", input);
      json!(true)
    }
    ("settings.setOpenAIConfig", "mutation") => {
      upsert_config(&mut settings.model_providers_config, "openAI", input);
      json!(true)
    }
    ("settings.setAnthropicConfig", "mutation") => {
      upsert_config(&mut settings.model_providers_config, "anthropic", input);
      json!(true)
    }
    ("settings.setGoogleConfig", "mutation") => {
      upsert_config(&mut settings.model_providers_config, "google", input);
      json!(true)
    }
    ("settings.setTranscriptionOpenAIConfig", "mutation") => {
      upsert_config(&mut settings.transcription_providers_config, "openAI", input);
      json!(true)
    }
    ("settings.setTranscriptionGroqConfig", "mutation") => {
      upsert_config(&mut settings.transcription_providers_config, "groq", input);
      json!(true)
    }
    ("settings.setTranscriptionGrokConfig", "mutation") => {
      upsert_config(&mut settings.transcription_providers_config, "grok", input);
      json!(true)
    }
    ("settings.removeOpenRouterProvider", "mutation") => {
      settings.model_providers_config.remove("openRouter");
      json!(true)
    }
    ("settings.removeOllamaProvider", "mutation") => {
      settings.model_providers_config.remove("ollama");
      json!(true)
    }
    ("settings.removeOpenAIProvider", "mutation") => {
      settings.model_providers_config.remove("openAI");
      json!(true)
    }
    ("settings.removeAnthropicProvider", "mutation") => {
      settings.model_providers_config.remove("anthropic");
      json!(true)
    }
    ("settings.removeGoogleProvider", "mutation") => {
      settings.model_providers_config.remove("google");
      json!(true)
    }
    ("settings.removeTranscriptionOpenAIProvider", "mutation") => {
      settings.transcription_providers_config.remove("openAI");
      json!(true)
    }
    ("settings.removeTranscriptionGroqProvider", "mutation") => {
      settings.transcription_providers_config.remove("groq");
      json!(true)
    }
    ("settings.removeTranscriptionGrokProvider", "mutation") => {
      settings.transcription_providers_config.remove("grok");
      json!(true)
    }
    ("settings.getModes", "query") => json!(get_modes_state(settings)),
    ("settings.getActiveMode", "query") => {
      let modes = get_modes_state(settings);
      let active = modes
        .items
        .iter()
        .find(|mode| mode.id == modes.active_mode_id)
        .cloned()
        .or_else(|| modes.items.first().cloned())
        .unwrap_or_else(|| build_fallback_mode(settings));
      json!(active)
    }
    ("settings.setActiveMode", "mutation") => {
      let mode_id = input
        .get("modeId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
      if mode_id.is_empty() {
        return Err("Mode id is required".to_string());
      }
      let mut modes = settings
        .modes
        .clone()
        .unwrap_or_else(|| get_modes_state(settings));
      if !modes.items.iter().any(|mode| mode.id == mode_id) {
        return Err(format!("Mode with id \"{mode_id}\" not found"));
      }
      modes.active_mode_id = mode_id.to_string();
      settings.modes = Some(modes);
      json!(true)
    }
    ("settings.createMode", "mutation") => {
      let name = input
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
      if name.is_empty() {
        return Err("Name is required".to_string());
      }
      let dictation =
        parse_mode_dictation(input.get("dictation")).ok_or("Invalid dictation config")?;
      let formatter_config =
        parse_formatter_config(input.get("formatterConfig")).ok_or("Invalid formatter config")?;
      let custom_instructions = input
        .get("customInstructions")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string());
      let speech_model_id = input
        .get("speechModelId")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string());
      let app_bindings = input.get("appBindings").and_then(|v| v.as_array()).map(to_string_vec);
      let mut modes = settings
        .modes
        .clone()
        .unwrap_or_else(|| get_modes_state(settings));
      if modes.items.len() >= 20 {
        return Err("Maximum number of modes (20) reached".to_string());
      }
      let now = now_iso();
      let new_mode = ModeConfigState {
        id: Uuid::new_v4().to_string(),
        name,
        is_default: false,
        dictation,
        formatter_config,
        custom_instructions,
        speech_model_id,
        app_bindings,
        created_at: now.clone(),
        updated_at: now,
      };
      modes.items.push(new_mode.clone());
      settings.modes = Some(modes);
      json!(new_mode)
    }
    ("settings.updateMode", "mutation") => {
      let mode_id = input
        .get("modeId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
      if mode_id.is_empty() {
        return Err("Mode id is required".to_string());
      }
      let mut modes = settings
        .modes
        .clone()
        .unwrap_or_else(|| get_modes_state(settings));
      let index = modes.items.iter().position(|mode| mode.id == mode_id);
      let index = index.ok_or_else(|| format!("Mode with id \"{mode_id}\" not found"))?;
      let mut updated = modes.items[index].clone();
      if let Some(name) = input.get("name").and_then(|v| v.as_str()) {
        updated.name = name.to_string();
      }
      if let Some(dictation) = parse_mode_dictation(input.get("dictation")) {
        updated.dictation = dictation;
      }
      if let Some(formatter_config) = parse_formatter_config(input.get("formatterConfig")) {
        updated.formatter_config = formatter_config;
      }
      if let Some(value) = input.get("customInstructions") {
        updated.custom_instructions = match value {
          Value::String(value) => Some(value.to_string()),
          Value::Null => None,
          _ => updated.custom_instructions.clone(),
        };
      }
      if let Some(value) = input.get("speechModelId") {
        updated.speech_model_id = match value {
          Value::String(value) => Some(value.to_string()),
          Value::Null => None,
          _ => updated.speech_model_id.clone(),
        };
      }
      if let Some(value) = input.get("appBindings") {
        updated.app_bindings = match value {
          Value::Array(values) => Some(to_string_vec(values)),
          Value::Null => None,
          _ => updated.app_bindings.clone(),
        };
      }
      updated.updated_at = now_iso();
      modes.items[index] = updated.clone();
      settings.modes = Some(modes);
      json!(updated)
    }
    ("settings.deleteMode", "mutation") => {
      let mode_id = input
        .get("modeId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
      if mode_id.is_empty() {
        return Err("Mode id is required".to_string());
      }
      let mut modes = settings
        .modes
        .clone()
        .unwrap_or_else(|| get_modes_state(settings));
      let mode = modes.items.iter().find(|m| m.id == mode_id);
      let mode = mode.ok_or_else(|| format!("Mode with id \"{mode_id}\" not found"))?;
      if mode.is_default {
        return Err("Cannot delete the default mode".to_string());
      }
      if modes.items.len() <= 1 {
        return Err("Cannot delete the last remaining mode".to_string());
      }
      modes.items.retain(|m| m.id != mode_id);
      if modes.active_mode_id == mode_id {
        modes.active_mode_id = "default".to_string();
      }
      settings.modes = Some(modes);
      json!(true)
    }
    ("settings.getInstalledApps", "query") => Value::Array(list_installed_apps()),
    ("recording.signalStart", "mutation") => {
      let mut session = recording
        .lock()
        .map_err(|_| "Failed to lock recording state".to_string())?;
      if session.state != "recording" {
        session.state = "recording".to_string();
        session.mode = "hands-free".to_string();
        session.audio_samples.clear();
        session.started_at = Some(now_unix_seconds());
        emit_trpc_event(
          app,
          "recording.stateUpdates",
          json!({ "state": session.state, "mode": session.mode }),
        );
        emit_trpc_event(app, "recording.voiceDetectionUpdates", json!(false));
      }
      json!(true)
    }
    ("recording.signalStop", "mutation") => {
      let samples = {
        let mut session = recording
          .lock()
          .map_err(|_| "Failed to lock recording state".to_string())?;
        if session.state != "recording" {
          return Ok(json!(true));
        }
        session.state = "stopping".to_string();
        emit_trpc_event(
          app,
          "recording.stateUpdates",
          json!({ "state": session.state, "mode": session.mode }),
        );
        let samples = std::mem::take(&mut session.audio_samples);
        session.state = "idle".to_string();
        session.mode = "idle".to_string();
        session.started_at = None;
        emit_trpc_event(
          app,
          "recording.stateUpdates",
          json!({ "state": session.state, "mode": session.mode }),
        );
        emit_trpc_event(app, "recording.voiceDetectionUpdates", json!(false));
        samples
      };

      if samples.is_empty() {
        return Ok(json!(true));
      }

      let wav_bytes = wav_bytes_from_f32(&samples, RECORDING_SAMPLE_RATE);
      let recordings_dir = app_data_dir.join("recordings");
      let _ = fs::create_dir_all(&recordings_dir);
      let filename = format!("recording-{}.wav", Uuid::new_v4());
      let file_path = recordings_dir.join(filename);
      fs::write(&file_path, &wav_bytes).map_err(|error| error.to_string())?;

      let selected_model = settings.models.selected_model.clone();
      let available = load_available_models()?;
      let model = if selected_model.is_empty() {
        None
      } else {
        find_available_model(&available, &selected_model)
      };
      let mut transcription_text = String::new();
      let mut transcription_error: Option<String> = None;
      if let Some(model) = model {
        let setup = model
          .get("setup")
          .and_then(|value| value.as_str())
          .unwrap_or_default();
        if setup == "api" {
          let provider = model
            .get("provider")
            .and_then(|value| value.as_str())
            .unwrap_or("");
          let model_id = model
            .get("apiModelId")
            .and_then(|value| value.as_str())
            .unwrap_or("");
          let api_key = match provider {
            "OpenAI" => provider_api_key(&settings.transcription_providers_config, "openAI"),
            "Groq" => provider_api_key(&settings.transcription_providers_config, "groq"),
            "Grok" => provider_api_key(&settings.transcription_providers_config, "grok"),
            _ => None,
          };
          if let (Some(api_key), Some(endpoint)) = (api_key, transcription_endpoint(provider)) {
            let language = settings.dictation.selected_language.as_str();
            match transcribe_with_api(&api_key, endpoint, model_id, &wav_bytes, Some(language)) {
              Ok(text) => transcription_text = text,
              Err(error) => {
                transcription_error = Some(error);
              }
            }
          } else {
            transcription_error = Some("Missing API credentials for transcription".to_string());
          }
        } else if setup == "amical" {
          transcription_error = Some("Amical Cloud transcription is not available in Tauri".to_string());
        } else {
          transcription_error = Some("Local transcription is not implemented in Tauri".to_string());
        }
      } else {
        transcription_error = Some("No transcription model selected".to_string());
      }

      let duration = (samples.len() as f64 / RECORDING_SAMPLE_RATE as f64).round() as i64;
      let now = now_unix_seconds();
      let conn = lock_db(db)?;
      let meta = transcription_error
        .as_ref()
        .map(|error| json!({ "error": error }))
        .and_then(|value| serde_json::to_string(&value).ok());
      conn
        .execute(
          "INSERT INTO transcriptions (text, timestamp, language, audio_file, confidence, duration, speech_model, formatting_model, meta, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
          params![
            transcription_text,
            now,
            settings.dictation.selected_language.clone(),
            file_path.to_string_lossy().to_string(),
            Option::<f64>::None,
            duration,
            if selected_model.is_empty() {
              None::<String>
            } else {
              Some(selected_model)
            },
            Option::<String>::None,
            meta,
            now,
            now
          ],
        )
        .map_err(|error| error.to_string())?;
      json!(true)
    }
    ("models.getModels", "query") => {
      let model_type = input.get("type").and_then(|v| v.as_str());
      let provider_filter = input.get("provider").and_then(|v| v.as_str());
      let selectable = input
        .get("selectable")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

      if model_type == Some("speech") {
        let available = load_available_models()?;
        let is_authenticated = settings
          .auth_status
          .as_ref()
          .map(|status| status.is_authenticated)
          .unwrap_or(false);
        let openai = provider_has_api_key(&settings.transcription_providers_config, "openAI");
        let groq = provider_has_api_key(&settings.transcription_providers_config, "groq");
        let grok = provider_has_api_key(&settings.transcription_providers_config, "grok");
        let mut models = Vec::new();

        for model in &available {
          let id = model
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
          let downloaded = settings.downloaded_speech_models.get(id);
          if selectable {
            let setup = model
              .get("setup")
              .and_then(|value| value.as_str())
              .unwrap_or_default();
            let provider = model
              .get("provider")
              .and_then(|value| value.as_str())
              .unwrap_or_default();
            let allowed = match setup {
              "amical" => is_authenticated,
              "api" => match provider {
                "OpenAI" => openai,
                "Groq" => groq,
                "Grok" => grok,
                _ => false,
              },
              _ => downloaded.is_some(),
            };
            if !allowed {
              continue;
            }
          }
          models.push(speech_model_value(model, downloaded));
        }

        Value::Array(models)
      } else {
        let mut models = settings.synced_provider_models.clone();
        if let Some(provider) = provider_filter {
          models = models
            .into_iter()
            .filter(|model| {
              model
                .get("provider")
                .and_then(|value| value.as_str())
                .map(|value| value == provider)
                .unwrap_or(false)
            })
            .collect();
        }
        if let Some(model_type) = model_type {
          let model_type = model_type.to_lowercase();
          models = models
            .into_iter()
            .filter(|model| {
              let provider = model
                .get("provider")
                .and_then(|value| value.as_str())
                .unwrap_or("");
              let name = model
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_lowercase();
              if model_type == "embedding" {
                provider == "Ollama" && name.contains("embed")
              } else if model_type == "language" {
                !(provider == "Ollama" && name.contains("embed"))
              } else {
                true
              }
            })
            .collect();
        }
        Value::Array(models)
      }
    }
    ("models.getAvailableModels", "query") => Value::Array(load_available_models()?),
    ("models.getDownloadedModels", "query") => {
      let available = load_available_models()?;
      let mut downloaded_map = Map::new();
      for (id, downloaded) in &settings.downloaded_speech_models {
        let value = if let Some(model) = find_available_model(&available, id) {
          speech_model_value(model, Some(downloaded))
        } else {
          json!({
            "id": id,
            "name": id,
            "provider": "local-whisper",
            "type": "speech",
            "size": Value::Null,
            "context": Value::Null,
            "description": "",
            "localPath": downloaded.local_path.clone(),
            "sizeBytes": downloaded.size_bytes,
            "checksum": downloaded.checksum.clone(),
            "downloadedAt": to_millis(downloaded.downloaded_at),
            "originalModel": Value::Null,
            "speed": Value::Null,
            "accuracy": Value::Null,
            "createdAt": to_millis(downloaded.downloaded_at),
            "updatedAt": to_millis(downloaded.downloaded_at),
            "setup": "offline"
          })
        };
        downloaded_map.insert(id.clone(), value);
      }
      Value::Object(downloaded_map)
    }
    ("models.getSyncedProviderModels", "query") => {
      Value::Array(settings.synced_provider_models.clone())
    }
    ("models.getActiveDownloads", "query") => json!([]),
    ("models.getDefaultModel", "query") => {
      let model_type = input.get("type").and_then(|v| v.as_str()).unwrap_or("");
      let value = match model_type {
        "speech" => &settings.models.default_speech_model,
        "language" => &settings.models.default_language_model,
        "embedding" => &settings.models.default_embedding_model,
        _ => "",
      };
      if value.is_empty() {
        Value::Null
      } else {
        json!(value)
      }
    }
    ("models.setDefaultModel", "mutation") => {
      let model_type = input.get("type").and_then(|v| v.as_str()).unwrap_or("");
      let model_id = input.get("modelId").and_then(|v| v.as_str()).unwrap_or("");
      match model_type {
        "speech" => settings.models.default_speech_model = model_id.to_string(),
        "language" => settings.models.default_language_model = model_id.to_string(),
        "embedding" => settings.models.default_embedding_model = model_id.to_string(),
        _ => {}
      }
      emit_trpc_event(
        app,
        "models.onSelectionChanged",
        json!({ "type": model_type, "modelId": model_id }),
      );
      json!(true)
    }
    ("models.getDefaultLanguageModel", "query") => {
      if settings.models.default_language_model.is_empty() {
        Value::Null
      } else {
        json!(settings.models.default_language_model)
      }
    }
    ("models.getDefaultEmbeddingModel", "query") => {
      if settings.models.default_embedding_model.is_empty() {
        Value::Null
      } else {
        json!(settings.models.default_embedding_model)
      }
    }
    ("models.setDefaultLanguageModel", "mutation") => {
      match input.get("modelId") {
        Some(Value::String(model_id)) => {
          settings.models.default_language_model = model_id.to_string();
          emit_trpc_event(
            app,
            "models.onSelectionChanged",
            json!({ "type": "language", "modelId": model_id }),
          );
        }
        Some(Value::Null) => {
          settings.models.default_language_model.clear();
        }
        _ => {}
      }
      json!(true)
    }
    ("models.setDefaultEmbeddingModel", "mutation") => {
      match input.get("modelId") {
        Some(Value::String(model_id)) => {
          settings.models.default_embedding_model = model_id.to_string();
          emit_trpc_event(
            app,
            "models.onSelectionChanged",
            json!({ "type": "embedding", "modelId": model_id }),
          );
        }
        Some(Value::Null) => {
          settings.models.default_embedding_model.clear();
        }
        _ => {}
      }
      json!(true)
    }
    ("models.getSelectedModel", "query") => {
      if settings.models.selected_model.is_empty() {
        Value::Null
      } else {
        json!(settings.models.selected_model)
      }
    }
    ("models.setSelectedModel", "mutation") => {
      match input.get("modelId") {
        Some(Value::String(model_id)) => {
          settings.models.selected_model = model_id.to_string();
          emit_trpc_event(
            app,
            "models.onSelectionChanged",
            json!({ "type": "speech", "modelId": model_id }),
          );
        }
        Some(Value::Null) => {
          settings.models.selected_model.clear();
        }
        _ => {}
      }
      json!(true)
    }
    ("models.getTranscriptionProviderStatus", "query") => json!({
      "openAI": provider_has_api_key(&settings.transcription_providers_config, "openAI"),
      "groq": provider_has_api_key(&settings.transcription_providers_config, "groq"),
      "grok": provider_has_api_key(&settings.transcription_providers_config, "grok")
    }),
    ("models.isTranscriptionAvailable", "query") => {
      let selected = settings.models.selected_model.clone();
      if selected.is_empty() {
        json!(false)
      } else {
        let available = load_available_models()?;
        let model = find_available_model(&available, &selected);
        let available = match model {
          Some(model) => {
            let setup = model
              .get("setup")
              .and_then(|value| value.as_str())
              .unwrap_or_default();
            let provider = model
              .get("provider")
              .and_then(|value| value.as_str())
              .unwrap_or_default();
            match setup {
              "amical" => settings
                .auth_status
                .as_ref()
                .map(|status| status.is_authenticated)
                .unwrap_or(false),
              "api" => match provider {
                "OpenAI" => provider_has_api_key(&settings.transcription_providers_config, "openAI"),
                "Groq" => provider_has_api_key(&settings.transcription_providers_config, "groq"),
                "Grok" => provider_has_api_key(&settings.transcription_providers_config, "grok"),
                _ => false,
              },
              _ => settings.downloaded_speech_models.contains_key(&selected),
            }
          }
          None => false,
        };
        json!(available)
      }
    }
    ("models.downloadModel", "mutation") => {
      let model_id = input
        .get("modelId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
      if model_id.is_empty() {
        return Err("Model id is required".to_string());
      }
      let available = load_available_models()?;
      let model = find_available_model(&available, model_id)
        .ok_or_else(|| format!("Model not found: {model_id}"))?;
      let setup = model
        .get("setup")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
      if setup != "offline" {
        return Err(format!("Model {model_id} is not a downloadable offline model"));
      }
      if settings.downloaded_speech_models.contains_key(model_id) {
        return Err(format!("Model already downloaded: {model_id}"));
      }
      let filename = model
        .get("filename")
        .and_then(|value| value.as_str())
        .unwrap_or(model_id);
      let models_dir = app_data_dir.join("models");
      let _ = fs::create_dir_all(&models_dir);
      let local_path = models_dir.join(filename).to_string_lossy().to_string();
      let size_bytes = model
        .get("size")
        .and_then(|value| value.as_f64())
        .map(|value| value as i64);
      let checksum = model
        .get("checksum")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
      settings.downloaded_speech_models.insert(
        model_id.to_string(),
        DownloadedSpeechModel {
          downloaded_at: now_unix_seconds(),
          size_bytes,
          checksum,
          local_path: Some(local_path),
        },
      );
      if let Some(total_bytes) = size_bytes {
        emit_trpc_event(
          app,
          "models.onDownloadProgress",
          json!({
            "modelId": model_id,
            "progress": {
              "modelId": model_id,
              "progress": 100,
              "status": "downloading",
              "bytesDownloaded": total_bytes,
              "totalBytes": total_bytes
            }
          }),
        );
      }
      emit_trpc_event(app, "models.onDownloadComplete", json!({ "modelId": model_id }));
      json!(true)
    }
    ("models.cancelDownload", "mutation") => {
      if let Some(model_id) = input.get("modelId").and_then(|v| v.as_str()) {
        emit_trpc_event(app, "models.onDownloadCancelled", json!({ "modelId": model_id }));
      }
      json!(true)
    }
    ("models.deleteModel", "mutation") => {
      let model_id = input
        .get("modelId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
      if model_id.is_empty() {
        return Err("Model id is required".to_string());
      }
      let removed = settings.downloaded_speech_models.remove(model_id);
      if let Some(removed) = removed {
        if let Some(path) = removed.local_path {
          let _ = fs::remove_file(path);
        }
      }
      if settings.models.selected_model == model_id {
        let preferred = [
          "whisper-large-v3-turbo",
          "whisper-large-v1",
          "whisper-medium",
          "whisper-small",
          "whisper-base",
          "whisper-tiny",
        ];
        let mut selected = String::new();
        for candidate in preferred {
          if settings.downloaded_speech_models.contains_key(candidate) {
            selected = candidate.to_string();
            break;
          }
        }
        settings.models.selected_model = selected;
      }
      if settings.models.default_speech_model == model_id {
        settings.models.default_speech_model.clear();
      }
      emit_trpc_event(app, "models.onModelDeleted", json!({ "modelId": model_id }));
      json!(true)
    }
    ("models.validateOpenRouterConnection", "mutation")
    | ("models.validateOpenAIConnection", "mutation")
    | ("models.validateAnthropicConnection", "mutation")
    | ("models.validateGoogleConnection", "mutation") => {
      let key = input.get("apiKey").and_then(|v| v.as_str()).unwrap_or("");
      if key.trim().is_empty() {
        json!({ "success": false, "error": "API key is required" })
      } else {
        json!({ "success": true })
      }
    }
    ("models.validateOllamaConnection", "mutation") => {
      let url = input.get("url").and_then(|v| v.as_str()).unwrap_or("");
      if url.trim().is_empty() {
        json!({ "success": false, "error": "URL is required" })
      } else {
        json!({ "success": true })
      }
    }
    ("models.validateTranscriptionOpenAIConnection", "mutation")
    | ("models.validateTranscriptionGroqConnection", "mutation")
    | ("models.validateTranscriptionGrokConnection", "mutation") => {
      let key = input.get("apiKey").and_then(|v| v.as_str()).unwrap_or("");
      if key.trim().is_empty() {
        json!({ "success": false, "error": "API key is required" })
      } else {
        json!({ "success": true })
      }
    }
    ("models.fetchOpenRouterModels", "query")
    | ("models.fetchOllamaModels", "query")
    | ("models.fetchOpenAIModels", "query")
    | ("models.fetchAnthropicModels", "query")
    | ("models.fetchGoogleModels", "query") => Value::Array(Vec::new()),
    ("models.syncProviderModelsToDatabase", "mutation") => {
      let provider = input
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("");
      let models = input.get("models").and_then(|v| v.as_array()).cloned().unwrap_or_default();
      let mut existing = settings.synced_provider_models.clone();
      existing.retain(|model| {
        model
          .get("provider")
          .and_then(|value| value.as_str())
          .map(|value| value != provider)
          .unwrap_or(true)
      });
      existing.extend(models);
      settings.synced_provider_models = existing;
      clear_missing_provider_defaults(settings);
      json!(true)
    }
    ("models.removeProviderModel", "mutation") => {
      let model_id = input
        .get("modelId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
      if model_id.is_empty() {
        return Err("Model id is required".to_string());
      }
      let before_len = settings.synced_provider_models.len();
      settings.synced_provider_models.retain(|model| {
        model
          .get("id")
          .and_then(|value| value.as_str())
          .map(|value| value != model_id)
          .unwrap_or(true)
      });
      clear_missing_provider_defaults(settings);
      if before_len == settings.synced_provider_models.len() {
        return Err(format!("Model not found: {model_id}"));
      }
      json!(true)
    }
    ("models.removeOpenRouterProvider", "mutation") => {
      settings.model_providers_config.remove("openRouter");
      settings.synced_provider_models.retain(|model| {
        model
          .get("provider")
          .and_then(|value| value.as_str())
          .map(|value| value != "OpenRouter")
          .unwrap_or(true)
      });
      clear_missing_provider_defaults(settings);
      json!(true)
    }
    ("models.removeOllamaProvider", "mutation") => {
      settings.model_providers_config.remove("ollama");
      settings.synced_provider_models.retain(|model| {
        model
          .get("provider")
          .and_then(|value| value.as_str())
          .map(|value| value != "Ollama")
          .unwrap_or(true)
      });
      clear_missing_provider_defaults(settings);
      json!(true)
    }
    ("models.removeOpenAIProvider", "mutation") => {
      settings.model_providers_config.remove("openAI");
      settings.synced_provider_models.retain(|model| {
        model
          .get("provider")
          .and_then(|value| value.as_str())
          .map(|value| value != "OpenAI")
          .unwrap_or(true)
      });
      clear_missing_provider_defaults(settings);
      json!(true)
    }
    ("models.removeAnthropicProvider", "mutation") => {
      settings.model_providers_config.remove("anthropic");
      settings.synced_provider_models.retain(|model| {
        model
          .get("provider")
          .and_then(|value| value.as_str())
          .map(|value| value != "Anthropic")
          .unwrap_or(true)
      });
      clear_missing_provider_defaults(settings);
      json!(true)
    }
    ("models.removeGoogleProvider", "mutation") => {
      settings.model_providers_config.remove("google");
      settings.synced_provider_models.retain(|model| {
        model
          .get("provider")
          .and_then(|value| value.as_str())
          .map(|value| value != "Google")
          .unwrap_or(true)
      });
      clear_missing_provider_defaults(settings);
      json!(true)
    }
    ("models.removeTranscriptionOpenAIProvider", "mutation") => {
      settings.transcription_providers_config.remove("openAI");
      let selected = settings.models.selected_model.clone();
      if !selected.is_empty() {
        if let Ok(models) = load_available_models() {
          if let Some(model) = find_available_model(&models, &selected) {
            let setup = model.get("setup").and_then(|v| v.as_str()).unwrap_or("");
            let provider = model.get("provider").and_then(|v| v.as_str()).unwrap_or("");
            if setup == "api" && provider == "OpenAI" {
              settings.models.selected_model.clear();
            }
          }
        }
      }
      json!(true)
    }
    ("models.removeTranscriptionGroqProvider", "mutation") => {
      settings.transcription_providers_config.remove("groq");
      let selected = settings.models.selected_model.clone();
      if !selected.is_empty() {
        if let Ok(models) = load_available_models() {
          if let Some(model) = find_available_model(&models, &selected) {
            let setup = model.get("setup").and_then(|v| v.as_str()).unwrap_or("");
            let provider = model.get("provider").and_then(|v| v.as_str()).unwrap_or("");
            if setup == "api" && provider == "Groq" {
              settings.models.selected_model.clear();
            }
          }
        }
      }
      json!(true)
    }
    ("models.removeTranscriptionGrokProvider", "mutation") => {
      settings.transcription_providers_config.remove("grok");
      let selected = settings.models.selected_model.clone();
      if !selected.is_empty() {
        if let Ok(models) = load_available_models() {
          if let Some(model) = find_available_model(&models, &selected) {
            let setup = model.get("setup").and_then(|v| v.as_str()).unwrap_or("");
            let provider = model.get("provider").and_then(|v| v.as_str()).unwrap_or("");
            if setup == "api" && provider == "Grok" {
              settings.models.selected_model.clear();
            }
          }
        }
      }
      json!(true)
    }
    ("notes.getNotes", "query") => {
      let conn = lock_db(db)?;
      let limit = input.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
      let offset = input.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
      let sort_by = input
        .get("sortBy")
        .and_then(|v| v.as_str())
        .unwrap_or("updatedAt");
      let sort_order = input
        .get("sortOrder")
        .and_then(|v| v.as_str())
        .unwrap_or("desc");
      let search = input
        .get("search")
        .and_then(|v| v.as_str())
        .filter(|value| !value.is_empty());
      let notes = list_notes(&conn, limit, offset, sort_by, sort_order, search)?;
      let values = notes.iter().map(note_row_to_value).collect();
      Value::Array(values)
    }
    ("notes.searchNotes", "query") => {
      let conn = lock_db(db)?;
      let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
      let limit = input.get("limit").and_then(|v| v.as_i64()).unwrap_or(10);
      let notes = list_notes(&conn, limit, 0, "updatedAt", "desc", Some(query))?;
      let results = notes
        .into_iter()
        .map(|note| {
          json!({
            "id": note.id,
            "title": note.title,
            "createdAt": to_millis(note.created_at),
            "icon": note.icon
          })
        })
        .collect();
      Value::Array(results)
    }
    ("notes.getNoteById", "query") => {
      let conn = lock_db(db)?;
      let id = input
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("Missing note id")?;
      let note = fetch_note_row(&conn, id)?.ok_or("Note not found".to_string())?;
      note_row_to_value(&note)
    }
    ("notes.createNote", "mutation") => {
      let mut conn = lock_db(db)?;
      let title = input
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
      if title.is_empty() {
        return Err("Title is required".to_string());
      }
      let initial_content = input
        .get("initialContent")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
      let icon = input
        .get("icon")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string());
      let now = now_unix_seconds();
      let tx = conn
        .transaction()
        .map_err(|error| error.to_string())?;
      tx.execute(
        "INSERT INTO notes (title, content, icon, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![title, "", icon, now, now],
      )
      .map_err(|error| error.to_string())?;
      let id = tx.last_insert_rowid();
      if !initial_content.is_empty() {
        let update = yjs_update_from_text(&initial_content)?;
        tx.execute(
          "INSERT INTO yjs_updates (note_id, update_data, created_at) VALUES (?1, ?2, ?3)",
          params![id, update, now],
        )
        .map_err(|error| error.to_string())?;
      }
      tx.commit().map_err(|error| error.to_string())?;
      let note = fetch_note_row(&conn, id)?.ok_or("Failed to load note".to_string())?;
      note_row_to_value(&note)
    }
    ("notes.updateNoteTitle", "mutation") => {
      let conn = lock_db(db)?;
      let id = input
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("Missing note id")?;
      let title = input
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
      if title.is_empty() {
        return Err("Title is required".to_string());
      }
      let now = now_unix_seconds();
      let updated = conn
        .execute(
          "UPDATE notes SET title = ?1, updated_at = ?2 WHERE id = ?3",
          params![title, now, id],
        )
        .map_err(|error| error.to_string())?;
      if updated == 0 {
        return Err("Note not found".to_string());
      }
      let note = fetch_note_row(&conn, id)?.ok_or("Note not found".to_string())?;
      note_row_to_value(&note)
    }
    ("notes.updateNoteIcon", "mutation") => {
      let conn = lock_db(db)?;
      let id = input
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("Missing note id")?;
      let icon = input
        .get("icon")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string());
      let now = now_unix_seconds();
      let updated = conn
        .execute(
          "UPDATE notes SET icon = ?1, updated_at = ?2 WHERE id = ?3",
          params![icon, now, id],
        )
        .map_err(|error| error.to_string())?;
      if updated == 0 {
        return Err("Note not found".to_string());
      }
      let note = fetch_note_row(&conn, id)?.ok_or("Note not found".to_string())?;
      note_row_to_value(&note)
    }
    ("notes.deleteNote", "mutation") => {
      let conn = lock_db(db)?;
      let id = input
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("Missing note id")?;
      let deleted = conn
        .execute("DELETE FROM notes WHERE id = ?1", params![id])
        .map_err(|error| error.to_string())?;
      if deleted == 0 {
        return Err("Note not found".to_string());
      }
      json!({ "success": true })
    }
    ("transcriptions.getTranscriptions", "query") => {
      let conn = lock_db(db)?;
      let limit = input.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
      let offset = input.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
      let sort_by = input
        .get("sortBy")
        .and_then(|v| v.as_str())
        .unwrap_or("timestamp");
      let sort_order = input
        .get("sortOrder")
        .and_then(|v| v.as_str())
        .unwrap_or("desc");
      let search = input
        .get("search")
        .and_then(|v| v.as_str())
        .filter(|value| !value.is_empty());
      let transcriptions =
        list_transcriptions(&conn, limit, offset, sort_by, sort_order, search)?;
      let values = transcriptions
        .iter()
        .map(transcription_row_to_value)
        .collect();
      Value::Array(values)
    }
    ("transcriptions.getTranscriptionsCount", "query") => {
      let conn = lock_db(db)?;
      let search = input
        .get("search")
        .and_then(|v| v.as_str())
        .filter(|value| !value.is_empty());
      json!(count_transcriptions(&conn, search)?)
    }
    ("transcriptions.getTranscriptionById", "query") => {
      let conn = lock_db(db)?;
      let id = input
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("Missing transcription id")?;
      if let Some(transcription) = fetch_transcription_row(&conn, id)? {
        transcription_row_to_value(&transcription)
      } else {
        Value::Null
      }
    }
    ("transcriptions.searchTranscriptions", "query") => {
      let conn = lock_db(db)?;
      let search_term = input
        .get("searchTerm")
        .and_then(|v| v.as_str())
        .unwrap_or("");
      let limit = input.get("limit").and_then(|v| v.as_i64()).unwrap_or(20);
      let transcriptions =
        list_transcriptions(&conn, limit, 0, "timestamp", "desc", Some(search_term))?;
      let values = transcriptions
        .iter()
        .map(transcription_row_to_value)
        .collect();
      Value::Array(values)
    }
    ("transcriptions.createTranscription", "mutation") => {
      let conn = lock_db(db)?;
      let text = input
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
      let timestamp = parse_timestamp_seconds(input.get("timestamp"))
        .unwrap_or_else(now_unix_seconds);
      let language = input
        .get("language")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "en".to_string());
      let audio_file = input
        .get("audioFile")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string());
      let confidence = input.get("confidence").and_then(|v| v.as_f64());
      let duration = input.get("duration").and_then(|v| v.as_i64());
      let speech_model = input
        .get("speechModel")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string());
      let formatting_model = input
        .get("formattingModel")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string());
      let meta = input
        .get("meta")
        .and_then(|value| serde_json::to_string(value).ok());
      let now = now_unix_seconds();
      conn
        .execute(
          "INSERT INTO transcriptions (text, timestamp, language, audio_file, confidence, duration, speech_model, formatting_model, meta, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
          params![
            text,
            timestamp,
            language,
            audio_file,
            confidence,
            duration,
            speech_model,
            formatting_model,
            meta,
            now,
            now
          ],
        )
        .map_err(|error| error.to_string())?;
      let id = conn.last_insert_rowid();
      let transcription = fetch_transcription_row(&conn, id)?
        .ok_or("Failed to load transcription".to_string())?;
      transcription_row_to_value(&transcription)
    }
    ("transcriptions.updateTranscription", "mutation") => {
      let conn = lock_db(db)?;
      let id = input
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("Missing transcription id")?;
      let data = input.get("data").unwrap_or(&Value::Null);
      let existing =
        fetch_transcription_row(&conn, id)?.ok_or("Transcription not found".to_string())?;
      let text = data
        .get("text")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string())
        .unwrap_or_else(|| existing.text.clone());
      let timestamp = if data.get("timestamp").is_some() {
        parse_timestamp_seconds(data.get("timestamp")).unwrap_or(existing.timestamp)
      } else {
        existing.timestamp
      };
      let language = if data.get("language").is_some() {
        data
          .get("language")
          .and_then(|v| v.as_str())
          .map(|value| value.to_string())
      } else {
        existing.language.clone()
      };
      let audio_file = if data.get("audioFile").is_some() {
        data
          .get("audioFile")
          .and_then(|v| v.as_str())
          .map(|value| value.to_string())
      } else {
        existing.audio_file.clone()
      };
      let confidence = if data.get("confidence").is_some() {
        data.get("confidence").and_then(|v| v.as_f64())
      } else {
        existing.confidence
      };
      let duration = if data.get("duration").is_some() {
        data.get("duration").and_then(|v| v.as_i64())
      } else {
        existing.duration
      };
      let speech_model = if data.get("speechModel").is_some() {
        data
          .get("speechModel")
          .and_then(|v| v.as_str())
          .map(|value| value.to_string())
      } else {
        existing.speech_model.clone()
      };
      let formatting_model = if data.get("formattingModel").is_some() {
        data
          .get("formattingModel")
          .and_then(|v| v.as_str())
          .map(|value| value.to_string())
      } else {
        existing.formatting_model.clone()
      };
      let meta = if data.get("meta").is_some() {
        data
          .get("meta")
          .and_then(|value| serde_json::to_string(value).ok())
      } else {
        existing.meta.clone()
      };
      let now = now_unix_seconds();
      conn
        .execute(
          "UPDATE transcriptions SET text = ?1, timestamp = ?2, language = ?3, audio_file = ?4, confidence = ?5, duration = ?6, speech_model = ?7, formatting_model = ?8, meta = ?9, updated_at = ?10 WHERE id = ?11",
          params![
            text,
            timestamp,
            language,
            audio_file,
            confidence,
            duration,
            speech_model,
            formatting_model,
            meta,
            now,
            id
          ],
        )
        .map_err(|error| error.to_string())?;
      let transcription = fetch_transcription_row(&conn, id)?
        .ok_or("Transcription not found".to_string())?;
      transcription_row_to_value(&transcription)
    }
    ("transcriptions.deleteTranscription", "mutation") => {
      let conn = lock_db(db)?;
      let id = input
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("Missing transcription id")?;
      let transcription =
        fetch_transcription_row(&conn, id)?.ok_or("Transcription not found".to_string())?;
      conn
        .execute("DELETE FROM transcriptions WHERE id = ?1", params![id])
        .map_err(|error| error.to_string())?;
      if let Some(audio_file) = &transcription.audio_file {
        if let Err(error) = fs::remove_file(audio_file) {
          eprintln!("Failed to delete audio file: {error}");
        }
      }
      transcription_row_to_value(&transcription)
    }
    ("transcriptions.getAudioFile", "mutation") => {
      let conn = lock_db(db)?;
      let id = input
        .get("transcriptionId")
        .and_then(|v| v.as_i64())
        .ok_or("Missing transcription id")?;
      let transcription =
        fetch_transcription_row(&conn, id)?.ok_or("Transcription not found".to_string())?;
      let audio_file = transcription
        .audio_file
        .ok_or("No audio file associated with this transcription".to_string())?;
      let audio_path = Path::new(&audio_file);
      let data = fs::read(audio_path)
        .map_err(|_| "Audio file not found or inaccessible".to_string())?;
      let filename = audio_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("audio")
        .to_string();
      let mime_type = audio_mime_type(audio_path);
      json!({
        "data": BASE64_ENGINE.encode(data),
        "filename": filename,
        "mimeType": mime_type
      })
    }
    ("transcriptions.downloadAudioFile", "mutation") => {
      let conn = lock_db(db)?;
      let id = input
        .get("transcriptionId")
        .and_then(|v| v.as_i64())
        .ok_or("Missing transcription id")?;
      let transcription =
        fetch_transcription_row(&conn, id)?.ok_or("Transcription not found".to_string())?;
      let audio_file = transcription
        .audio_file
        .ok_or("No audio file associated with this transcription".to_string())?;
      let audio_path = Path::new(&audio_file);
      let filename = audio_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("audio.wav");
      let dialog = app
        .dialog()
        .file()
        .set_file_name(filename.to_string())
        .add_filter("WAV", &["wav"])
        .add_filter("All Files", &["*"]);
      let destination = dialog.blocking_save_file();
      let destination = match destination {
        Some(path) => path
          .into_path()
          .map_err(|error| error.to_string())?,
        None => {
          return Ok(json!({
            "success": false,
            "canceled": true
          }));
        }
      };
      fs::create_dir_all(
        destination
          .parent()
          .unwrap_or_else(|| app_data_dir.as_path()),
      )
      .map_err(|error| error.to_string())?;
      fs::copy(audio_path, &destination).map_err(|error| error.to_string())?;
      json!({
        "success": true,
        "filePath": destination.to_string_lossy()
      })
    }
    ("vocabulary.getVocabulary", "query") => {
      let conn = lock_db(db)?;
      let limit = input.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
      let offset = input.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
      let sort_by = input
        .get("sortBy")
        .and_then(|v| v.as_str())
        .unwrap_or("dateAdded");
      let sort_order = input
        .get("sortOrder")
        .and_then(|v| v.as_str())
        .unwrap_or("desc");
      let search = input
        .get("search")
        .and_then(|v| v.as_str())
        .filter(|value| !value.is_empty());
      let items = list_vocabulary(&conn, limit, offset, sort_by, sort_order, search)?;
      let values = items.iter().map(vocabulary_row_to_value).collect();
      Value::Array(values)
    }
    ("vocabulary.createVocabularyWord", "mutation") => {
      let conn = lock_db(db)?;
      let word = input
        .get("word")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
      if word.is_empty() {
        return Err("Word is required".to_string());
      }
      let is_replacement = input
        .get("isReplacement")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
      let replacement_word = input
        .get("replacementWord")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string());
      if is_replacement && replacement_word.is_none() {
        return Err("replacementWord is required when isReplacement is true".to_string());
      }
      let now = now_unix_seconds();
      conn
        .execute(
          "INSERT INTO vocabulary (word, replacement_word, is_replacement, date_added, usage_count, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
          params![
            word,
            replacement_word,
            if is_replacement { 1 } else { 0 },
            now,
            0,
            now,
            now
          ],
        )
        .map_err(|error| error.to_string())?;
      let id = conn.last_insert_rowid();
      let item = fetch_vocabulary_row(&conn, id)?
        .ok_or("Failed to load vocabulary item".to_string())?;
      vocabulary_row_to_value(&item)
    }
    ("vocabulary.updateVocabulary", "mutation") => {
      let conn = lock_db(db)?;
      let id = input
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("Missing vocabulary id")?;
      let data = input.get("data").unwrap_or(&Value::Null);
      let existing =
        fetch_vocabulary_row(&conn, id)?.ok_or("Vocabulary item not found".to_string())?;
      let word = if data.get("word").is_some() {
        data
          .get("word")
          .and_then(|v| v.as_str())
          .unwrap_or("")
          .to_string()
      } else {
        existing.word.clone()
      };
      if word.is_empty() {
        return Err("Word is required".to_string());
      }
      let is_replacement = if data.get("isReplacement").is_some() {
        data
          .get("isReplacement")
          .and_then(|v| v.as_bool())
          .unwrap_or(false)
      } else {
        existing.is_replacement
      };
      let mut replacement_word = if data.get("replacementWord").is_some() {
        data
          .get("replacementWord")
          .and_then(|v| v.as_str())
          .map(|value| value.to_string())
      } else {
        existing.replacement_word.clone()
      };
      if is_replacement && replacement_word.is_none() {
        return Err("replacementWord is required when isReplacement is true".to_string());
      }
      if !is_replacement {
        replacement_word = None;
      }
      let now = now_unix_seconds();
      conn
        .execute(
          "UPDATE vocabulary SET word = ?1, replacement_word = ?2, is_replacement = ?3, updated_at = ?4 WHERE id = ?5",
          params![word, replacement_word, if is_replacement { 1 } else { 0 }, now, id],
        )
        .map_err(|error| error.to_string())?;
      let item = fetch_vocabulary_row(&conn, id)?
        .ok_or("Vocabulary item not found".to_string())?;
      vocabulary_row_to_value(&item)
    }
    ("vocabulary.deleteVocabulary", "mutation") => {
      let conn = lock_db(db)?;
      let id = input
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("Missing vocabulary id")?;
      let item =
        fetch_vocabulary_row(&conn, id)?.ok_or("Vocabulary item not found".to_string())?;
      conn
        .execute("DELETE FROM vocabulary WHERE id = ?1", params![id])
        .map_err(|error| error.to_string())?;
      vocabulary_row_to_value(&item)
    }
    ("onboarding.getFeatureFlags", "query") => json!({
      "skipWelcome": env_flag("ONBOARDING_SKIP_WELCOME"),
      "skipFeatures": env_flag("ONBOARDING_SKIP_FEATURES"),
      "skipDiscovery": env_flag("ONBOARDING_SKIP_DISCOVERY"),
      "skipModels": env_flag("ONBOARDING_SKIP_MODELS")
    }),
    ("onboarding.getSkippedScreens", "query") => {
      let mut skipped = Vec::new();
      if env_flag("ONBOARDING_SKIP_WELCOME") {
        skipped.push(Value::String("welcome".to_string()));
      }
      if env_flag("ONBOARDING_SKIP_FEATURES") {
        skipped.push(Value::String("features".to_string()));
      }
      if env_flag("ONBOARDING_SKIP_DISCOVERY") {
        skipped.push(Value::String("discovery".to_string()));
      }
      if env_flag("ONBOARDING_SKIP_MODELS") {
        skipped.push(Value::String("models".to_string()));
      }
      Value::Array(skipped)
    }
    ("onboarding.getState", "query") => match &settings.onboarding_state {
      Some(state) => json!(state),
      None => Value::Null,
    },
    ("onboarding.getSystemRecommendation", "query") => system_recommendation(),
    ("onboarding.getRecommendedLocalModel", "query") => {
      let cpu_model = get_system_specs()
        .and_then(|specs| specs.cpu_model)
        .unwrap_or_default();
      json!(recommended_local_model(&cpu_model))
    }
    ("onboarding.getPlatform", "query") => json!(current_platform()),
    ("onboarding.checkMicrophonePermission", "query") => json!("granted"),
    ("onboarding.checkAccessibilityPermission", "query") => json!(true),
    ("onboarding.requestMicrophonePermission", "mutation") => {
      if current_platform() == "darwin" {
        let _ = open_external_url(
          "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
        );
      }
      json!(true)
    }
    ("onboarding.requestAccessibilityPermission", "mutation") => {
      if current_platform() == "darwin" {
        let _ = open_external_url(
          "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        );
      }
      json!(true)
    }
    ("onboarding.openExternal", "mutation") => {
      if let Some(url) = input.get("url").and_then(|v| v.as_str()) {
        let _ = open_external_url(url);
      }
      json!(true)
    }
    ("onboarding.quitApp", "mutation") => {
      app.exit(0);
      Value::Null
    }
    ("onboarding.logError", "mutation") => {
      let message = input
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("");
      eprintln!("[Onboarding] {message}");
      Value::Null
    }
    ("onboarding.savePreferences", "mutation") => {
      let mut prefs = settings.onboarding_preferences.clone().unwrap_or_default();
      if let Some(values) = input.get("featureInterests").and_then(|v| v.as_array()) {
        prefs.feature_interests = Some(to_string_vec(values));
      }
      if let Some(value) = input.get("discoverySource").and_then(|v| v.as_str()) {
        prefs.discovery_source = Some(value.to_string());
      }
      if let Some(value) = input.get("discoveryDetails").and_then(|v| v.as_str()) {
        prefs.discovery_details = Some(value.to_string());
      }
      if let Some(value) = input.get("selectedModelType").and_then(|v| v.as_str()) {
        prefs.selected_model_type = Some(value.to_string());
      }
      if let Some(value) = input.get("lastVisitedScreen").and_then(|v| v.as_str()) {
        prefs.last_visited_screen = Some(value.to_string());
      }
      if let Some(value) = input.get("modelRecommendation") {
        prefs.model_recommendation = match value {
          Value::Object(map) => {
            let suggested = map.get("suggested").and_then(|v| v.as_str()).unwrap_or("");
            let reason = map.get("reason").and_then(|v| v.as_str()).unwrap_or("");
            let followed = map.get("followed").and_then(|v| v.as_bool()).unwrap_or(false);
            Some(OnboardingModelRecommendationState {
              suggested: suggested.to_string(),
              reason: reason.to_string(),
              followed,
            })
          }
          Value::Null => None,
          _ => prefs.model_recommendation.clone(),
        };
      }
      settings.onboarding_preferences = Some(prefs);
      json!({ "success": true })
    }
    ("onboarding.complete", "mutation") => {
      let state: OnboardingState =
        serde_json::from_value(input.clone()).map_err(|error| error.to_string())?;
      settings.onboarding_state = Some(state);
      json!({ "success": true })
    }
    ("onboarding.reset", "mutation") => {
      settings.onboarding_state = None;
      settings.onboarding_preferences = None;
      json!({ "success": true })
    }
    ("auth.getAuthStatus", "query") => {
      let status = settings.auth_status.clone().unwrap_or_default();
      json!({
        "isAuthenticated": status.is_authenticated,
        "userEmail": status.user_email,
        "userName": status.user_name
      })
    }
    ("auth.login", "mutation") => json!({
      "success": false,
      "message": "OAuth は未実装です。"
    }),
    ("auth.logout", "mutation") => {
      settings.auth_status = Some(AuthStatusState {
        is_authenticated: false,
        user_email: None,
        user_name: None,
      });
      emit_trpc_event(
        app,
        "auth.onAuthStateChange",
        json!({
          "isAuthenticated": false,
          "userEmail": Value::Null,
          "userName": Value::Null
        }),
      );
      json!({
        "success": true,
        "message": "Logged out successfully"
      })
    }
    ("auth.isCloudModelSelected", "query") => {
      let selected = settings.models.selected_model.clone();
      if selected.is_empty() {
        json!(false)
      } else {
        let available = load_available_models()?;
        let is_cloud = find_available_model(&available, &selected)
          .and_then(|model| model.get("setup"))
          .and_then(|value| value.as_str())
          .map(|value| value == "amical")
          .unwrap_or(false);
        json!(is_cloud)
      }
    }
    ("settings.downloadLogFile", "mutation") => {
      let log_path = log_file_path(app_data_dir);
      if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
      }
      if !log_path.exists() {
        let _ = fs::write(&log_path, "");
      }
      let filename = format!("amical-logs-{}.log", Local::now().format("%Y-%m-%d"));
      let dialog = app
        .dialog()
        .file()
        .set_file_name(filename)
        .add_filter("Log Files", &["log", "txt"])
        .add_filter("All Files", &["*"]);
      let destination = dialog.blocking_save_file();
      let destination = match destination {
        Some(path) => path
          .into_path()
          .map_err(|error| error.to_string())?,
        None => {
          return Ok(json!({
            "success": false,
            "canceled": true
          }));
        }
      };
      if let Err(error) = fs::copy(&log_path, &destination) {
        return Err(error.to_string());
      }
      json!({
        "success": true,
        "path": destination.to_string_lossy()
      })
    }
    ("settings.resetApp", "mutation") => {
      reset_app_state(app, app_data_dir, db)?;
      json!({ "success": true })
    }
    ("widget.setIgnoreMouseEvents", "mutation") => {
      let ignore = input.get("ignore").and_then(|v| v.as_bool()).unwrap_or(false);
      if !ignore {
        if let Some(widget) = app.get_webview_window("widget") {
          let _ = widget.set_ignore_cursor_events(false);
        }
      }
      json!(true)
    }
    ("widget.navigateMainWindow", "mutation") => {
      let route = input.get("route").and_then(|v| v.as_str()).unwrap_or("");
      if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        if !route.is_empty() {
          let _ = window.emit("navigate", route.to_string());
        }
      }
      json!(true)
    }
    ("updater.isCheckingForUpdate", "query") => json!(false),
    ("updater.isUpdateAvailable", "query") => json!(false),
    ("updater.checkForUpdates", "mutation") => json!({ "success": true }),
    ("updater.downloadUpdate", "mutation") => json!({ "success": true }),
    ("updater.quitAndInstall", "mutation") => {
      app.exit(0);
      json!({ "success": true })
    }
    (_, "mutation") => json!(true),
    _ => Value::Null,
  })
}

fn lock_db<'a>(
  db: &'a Mutex<Connection>,
) -> Result<std::sync::MutexGuard<'a, Connection>, String> {
  db.lock()
    .map_err(|_| "Failed to lock database".to_string())
}

fn now_unix_seconds() -> i64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|duration| duration.as_secs() as i64)
    .unwrap_or(0)
}

fn to_millis(value: i64) -> i64 {
  if value > 1_000_000_000_000 {
    value
  } else {
    value * 1000
  }
}

fn parse_timestamp_seconds(value: Option<&Value>) -> Option<i64> {
  match value {
    Some(Value::Number(number)) => number
      .as_i64()
      .map(|value| if value > 1_000_000_000_000 { value / 1000 } else { value }),
    Some(Value::String(value)) => DateTime::parse_from_rfc3339(value)
      .ok()
      .map(|timestamp| timestamp.timestamp()),
    _ => None,
  }
}

#[derive(Clone)]
struct NoteRow {
  id: i64,
  title: String,
  content: Option<String>,
  icon: Option<String>,
  created_at: i64,
  updated_at: i64,
}

fn note_row_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteRow> {
  Ok(NoteRow {
    id: row.get(0)?,
    title: row.get(1)?,
    content: row.get(2)?,
    icon: row.get(3)?,
    created_at: row.get(4)?,
    updated_at: row.get(5)?,
  })
}

fn note_row_to_value(note: &NoteRow) -> Value {
  json!({
    "id": note.id,
    "title": note.title.clone(),
    "content": note.content.clone().unwrap_or_default(),
    "icon": note.icon.clone(),
    "createdAt": to_millis(note.created_at),
    "updatedAt": to_millis(note.updated_at)
  })
}

fn fetch_note_row(conn: &Connection, id: i64) -> Result<Option<NoteRow>, String> {
  conn
    .query_row(
      "SELECT id, title, content, icon, created_at, updated_at FROM notes WHERE id = ?1",
      params![id],
      note_row_from_row,
    )
    .optional()
    .map_err(|error| error.to_string())
}

fn list_notes(
  conn: &Connection,
  limit: i64,
  offset: i64,
  sort_by: &str,
  sort_order: &str,
  search: Option<&str>,
) -> Result<Vec<NoteRow>, String> {
  let limit = limit.max(0);
  let offset = offset.max(0);
  let sort_column = match sort_by {
    "title" => "title",
    "createdAt" => "created_at",
    _ => "updated_at",
  };
  let order = if sort_order.eq_ignore_ascii_case("asc") {
    "ASC"
  } else {
    "DESC"
  };

  let sql = if search.is_some() {
    format!(
      "SELECT id, title, content, icon, created_at, updated_at FROM notes WHERE title LIKE ?1 COLLATE NOCASE ORDER BY {} {} LIMIT ?2 OFFSET ?3",
      sort_column, order
    )
  } else {
    format!(
      "SELECT id, title, content, icon, created_at, updated_at FROM notes ORDER BY {} {} LIMIT ?1 OFFSET ?2",
      sort_column, order
    )
  };
  let mut stmt = conn.prepare(&sql).map_err(|error| error.to_string())?;
  let rows = if let Some(search) = search {
    let pattern = format!("%{}%", search);
    stmt
      .query_map(params![pattern, limit, offset], note_row_from_row)
      .map_err(|error| error.to_string())?
  } else {
    stmt
      .query_map(params![limit, offset], note_row_from_row)
      .map_err(|error| error.to_string())?
  };

  let mut notes = Vec::new();
  for row in rows {
    notes.push(row.map_err(|error| error.to_string())?);
  }
  Ok(notes)
}

fn yjs_update_from_text(content: &str) -> Result<Vec<u8>, String> {
  let doc = Doc::new();
  let text = doc.get_or_insert_text("content");
  {
    let mut txn = doc.transact_mut();
    text.insert(&mut txn, 0, content);
  }
  let update = doc
    .transact()
    .encode_state_as_update_v1(&StateVector::default());
  Ok(update)
}

fn load_yjs_updates_for_note(conn: &Connection, note_id: i64) -> Result<Vec<Vec<u8>>, String> {
  let mut stmt = conn
    .prepare("SELECT update_data FROM yjs_updates WHERE note_id = ?1 ORDER BY id ASC")
    .map_err(|error| error.to_string())?;
  let rows = stmt
    .query_map(params![note_id], |row| row.get::<_, Vec<u8>>(0))
    .map_err(|error| error.to_string())?;
  let mut updates = Vec::new();
  for row in rows {
    updates.push(row.map_err(|error| error.to_string())?);
  }
  Ok(updates)
}

fn replace_yjs_updates_for_note(
  conn: &mut Connection,
  note_id: i64,
  update: &[u8],
) -> Result<(), String> {
  let now = now_unix_seconds();
  let tx = conn
    .transaction()
    .map_err(|error| error.to_string())?;
  tx.execute(
    "DELETE FROM yjs_updates WHERE note_id = ?1",
    params![note_id],
  )
  .map_err(|error| error.to_string())?;
  tx.execute(
    "INSERT INTO yjs_updates (note_id, update_data, created_at) VALUES (?1, ?2, ?3)",
    params![note_id, update, now],
  )
  .map_err(|error| error.to_string())?;
  tx.commit().map_err(|error| error.to_string())?;
  Ok(())
}

fn get_unique_note_ids(conn: &Connection) -> Result<Vec<i64>, String> {
  let mut stmt = conn
    .prepare("SELECT note_id FROM yjs_updates GROUP BY note_id")
    .map_err(|error| error.to_string())?;
  let rows = stmt
    .query_map([], |row| row.get::<_, i64>(0))
    .map_err(|error| error.to_string())?;
  let mut ids = Vec::new();
  for row in rows {
    ids.push(row.map_err(|error| error.to_string())?);
  }
  Ok(ids)
}

fn compact_note_updates(conn: &mut Connection, note_id: i64) -> Result<(usize, usize), String> {
  let updates = load_yjs_updates_for_note(conn, note_id)?;
  let updates_before = updates.len();
  if updates_before <= 1 {
    return Ok((updates_before, updates_before));
  }

  let doc = Doc::new();
  let mut txn = doc.transact_mut();
  for update in updates {
    let decoded = Update::decode_v1(&update).map_err(|error| error.to_string())?;
    txn.apply_update(decoded);
  }
  drop(txn);
  let compacted = doc
    .transact()
    .encode_state_as_update_v1(&StateVector::default());
  replace_yjs_updates_for_note(conn, note_id, &compacted)?;

  Ok((updates_before, 1))
}

fn compact_all_notes(conn: &mut Connection) -> Result<(), String> {
  let note_ids = get_unique_note_ids(conn)?;
  for note_id in note_ids {
    if let Err(error) = compact_note_updates(conn, note_id) {
      eprintln!("Failed to compact note {note_id}: {error}");
    }
  }
  Ok(())
}

#[derive(Clone)]
struct TranscriptionRow {
  id: i64,
  text: String,
  timestamp: i64,
  language: Option<String>,
  audio_file: Option<String>,
  confidence: Option<f64>,
  duration: Option<i64>,
  speech_model: Option<String>,
  formatting_model: Option<String>,
  meta: Option<String>,
  created_at: i64,
  updated_at: i64,
}

fn transcription_row_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TranscriptionRow> {
  Ok(TranscriptionRow {
    id: row.get(0)?,
    text: row.get(1)?,
    timestamp: row.get(2)?,
    language: row.get(3)?,
    audio_file: row.get(4)?,
    confidence: row.get(5)?,
    duration: row.get(6)?,
    speech_model: row.get(7)?,
    formatting_model: row.get(8)?,
    meta: row.get(9)?,
    created_at: row.get(10)?,
    updated_at: row.get(11)?,
  })
}

fn meta_value(meta: &Option<String>) -> Value {
  if let Some(value) = meta {
    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.clone()))
  } else {
    Value::Null
  }
}

fn transcription_row_to_value(row: &TranscriptionRow) -> Value {
  json!({
    "id": row.id,
    "text": row.text.clone(),
    "timestamp": to_millis(row.timestamp),
    "language": row.language.clone(),
    "audioFile": row.audio_file.clone(),
    "confidence": row.confidence,
    "duration": row.duration,
    "speechModel": row.speech_model.clone(),
    "formattingModel": row.formatting_model.clone(),
    "meta": meta_value(&row.meta),
    "createdAt": to_millis(row.created_at),
    "updatedAt": to_millis(row.updated_at)
  })
}

fn fetch_transcription_row(
  conn: &Connection,
  id: i64,
) -> Result<Option<TranscriptionRow>, String> {
  conn
    .query_row(
      "SELECT id, text, timestamp, language, audio_file, confidence, duration, speech_model, formatting_model, meta, created_at, updated_at FROM transcriptions WHERE id = ?1",
      params![id],
      transcription_row_from_row,
    )
    .optional()
    .map_err(|error| error.to_string())
}

fn list_transcriptions(
  conn: &Connection,
  limit: i64,
  offset: i64,
  sort_by: &str,
  sort_order: &str,
  search: Option<&str>,
) -> Result<Vec<TranscriptionRow>, String> {
  let limit = limit.max(0);
  let offset = offset.max(0);
  let sort_column = match sort_by {
    "createdAt" => "created_at",
    _ => "timestamp",
  };
  let order = if sort_order.eq_ignore_ascii_case("asc") {
    "ASC"
  } else {
    "DESC"
  };
  let sql = if search.is_some() {
    format!(
      "SELECT id, text, timestamp, language, audio_file, confidence, duration, speech_model, formatting_model, meta, created_at, updated_at FROM transcriptions WHERE text LIKE ?1 COLLATE NOCASE ORDER BY {} {} LIMIT ?2 OFFSET ?3",
      sort_column, order
    )
  } else {
    format!(
      "SELECT id, text, timestamp, language, audio_file, confidence, duration, speech_model, formatting_model, meta, created_at, updated_at FROM transcriptions ORDER BY {} {} LIMIT ?1 OFFSET ?2",
      sort_column, order
    )
  };
  let mut stmt = conn.prepare(&sql).map_err(|error| error.to_string())?;
  let rows = if let Some(search) = search {
    let pattern = format!("%{}%", search);
    stmt
      .query_map(params![pattern, limit, offset], transcription_row_from_row)
      .map_err(|error| error.to_string())?
  } else {
    stmt
      .query_map(params![limit, offset], transcription_row_from_row)
      .map_err(|error| error.to_string())?
  };

  let mut transcriptions = Vec::new();
  for row in rows {
    transcriptions.push(row.map_err(|error| error.to_string())?);
  }
  Ok(transcriptions)
}

fn count_transcriptions(conn: &Connection, search: Option<&str>) -> Result<i64, String> {
  if let Some(search) = search {
    let pattern = format!("%{}%", search);
    conn
      .query_row(
        "SELECT COUNT(*) FROM transcriptions WHERE text LIKE ?1 COLLATE NOCASE",
        params![pattern],
        |row| row.get(0),
      )
      .map_err(|error| error.to_string())
  } else {
    conn
      .query_row("SELECT COUNT(*) FROM transcriptions", [], |row| row.get(0))
      .map_err(|error| error.to_string())
  }
}

#[derive(Clone)]
struct VocabularyRow {
  id: i64,
  word: String,
  replacement_word: Option<String>,
  is_replacement: bool,
  date_added: i64,
  usage_count: i64,
  created_at: i64,
  updated_at: i64,
}

fn vocabulary_row_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<VocabularyRow> {
  let is_replacement: i64 = row.get(3)?;
  Ok(VocabularyRow {
    id: row.get(0)?,
    word: row.get(1)?,
    replacement_word: row.get(2)?,
    is_replacement: is_replacement != 0,
    date_added: row.get(4)?,
    usage_count: row.get(5)?,
    created_at: row.get(6)?,
    updated_at: row.get(7)?,
  })
}

fn vocabulary_row_to_value(row: &VocabularyRow) -> Value {
  json!({
    "id": row.id,
    "word": row.word.clone(),
    "replacementWord": row.replacement_word.clone(),
    "isReplacement": row.is_replacement,
    "dateAdded": to_millis(row.date_added),
    "usageCount": row.usage_count,
    "createdAt": to_millis(row.created_at),
    "updatedAt": to_millis(row.updated_at)
  })
}

fn fetch_vocabulary_row(conn: &Connection, id: i64) -> Result<Option<VocabularyRow>, String> {
  conn
    .query_row(
      "SELECT id, word, replacement_word, is_replacement, date_added, usage_count, created_at, updated_at FROM vocabulary WHERE id = ?1",
      params![id],
      vocabulary_row_from_row,
    )
    .optional()
    .map_err(|error| error.to_string())
}

fn list_vocabulary(
  conn: &Connection,
  limit: i64,
  offset: i64,
  sort_by: &str,
  sort_order: &str,
  search: Option<&str>,
) -> Result<Vec<VocabularyRow>, String> {
  let limit = limit.max(0);
  let offset = offset.max(0);
  let sort_column = match sort_by {
    "word" => "word",
    "usageCount" => "usage_count",
    _ => "date_added",
  };
  let order = if sort_order.eq_ignore_ascii_case("asc") {
    "ASC"
  } else {
    "DESC"
  };
  let sql = if search.is_some() {
    format!(
      "SELECT id, word, replacement_word, is_replacement, date_added, usage_count, created_at, updated_at FROM vocabulary WHERE word LIKE ?1 COLLATE NOCASE ORDER BY {} {} LIMIT ?2 OFFSET ?3",
      sort_column, order
    )
  } else {
    format!(
      "SELECT id, word, replacement_word, is_replacement, date_added, usage_count, created_at, updated_at FROM vocabulary ORDER BY {} {} LIMIT ?1 OFFSET ?2",
      sort_column, order
    )
  };
  let mut stmt = conn.prepare(&sql).map_err(|error| error.to_string())?;
  let rows = if let Some(search) = search {
    let pattern = format!("%{}%", search);
    stmt
      .query_map(params![pattern, limit, offset], vocabulary_row_from_row)
      .map_err(|error| error.to_string())?
  } else {
    stmt
      .query_map(params![limit, offset], vocabulary_row_from_row)
      .map_err(|error| error.to_string())?
  };

  let mut items = Vec::new();
  for row in rows {
    items.push(row.map_err(|error| error.to_string())?);
  }
  Ok(items)
}

fn audio_mime_type(path: &Path) -> &'static str {
  let ext = path
    .extension()
    .and_then(|value| value.to_str())
    .unwrap_or("")
    .to_lowercase();
  match ext.as_str() {
    "wav" => "audio/wav",
    "mp3" => "audio/mpeg",
    "webm" => "audio/webm",
    "ogg" => "audio/ogg",
    "m4a" => "audio/mp4",
    "flac" => "audio/flac",
    _ => "audio/wav",
  }
}

const RECORDING_SAMPLE_RATE: u32 = 16000;

fn wav_bytes_from_f32(samples: &[f32], sample_rate: u32) -> Vec<u8> {
  let num_channels: u16 = 1;
  let bits_per_sample: u16 = 16;
  let byte_rate: u32 = sample_rate * u32::from(num_channels) * u32::from(bits_per_sample) / 8;
  let block_align: u16 = num_channels * bits_per_sample / 8;
  let data_size: u32 = samples.len() as u32 * 2;
  let mut bytes = Vec::with_capacity(44 + data_size as usize);

  bytes.extend_from_slice(b"RIFF");
  bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
  bytes.extend_from_slice(b"WAVE");
  bytes.extend_from_slice(b"fmt ");
  bytes.extend_from_slice(&16u32.to_le_bytes());
  bytes.extend_from_slice(&1u16.to_le_bytes());
  bytes.extend_from_slice(&num_channels.to_le_bytes());
  bytes.extend_from_slice(&sample_rate.to_le_bytes());
  bytes.extend_from_slice(&byte_rate.to_le_bytes());
  bytes.extend_from_slice(&block_align.to_le_bytes());
  bytes.extend_from_slice(&bits_per_sample.to_le_bytes());
  bytes.extend_from_slice(b"data");
  bytes.extend_from_slice(&data_size.to_le_bytes());

  for sample in samples {
    let clamped = sample.clamp(-1.0, 1.0);
    let value = (clamped * i16::MAX as f32) as i16;
    bytes.extend_from_slice(&value.to_le_bytes());
  }

  bytes
}

fn provider_api_key(config: &Map<String, Value>, key: &str) -> Option<String> {
  config
    .get(key)
    .and_then(|value| value.get("apiKey"))
    .and_then(|value| value.as_str())
    .map(|value| value.to_string())
}

fn transcription_endpoint(provider: &str) -> Option<&'static str> {
  match provider {
    "OpenAI" => Some("https://api.openai.com/v1/audio/transcriptions"),
    "Groq" => Some("https://api.groq.com/openai/v1/audio/transcriptions"),
    "Grok" => Some("https://api.x.ai/v1/audio/transcriptions"),
    _ => None,
  }
}

fn transcribe_with_api(
  api_key: &str,
  endpoint: &str,
  model: &str,
  wav_bytes: &[u8],
  language: Option<&str>,
) -> Result<String, String> {
  let part = Part::bytes(wav_bytes.to_vec())
    .file_name("audio.wav")
    .mime_str("audio/wav")
    .map_err(|error| error.to_string())?;
  let mut form = Form::new().part("file", part).text("model", model.to_string());
  if let Some(language) = language {
    if !language.is_empty() && language != "auto" {
      form = form.text("language", language.to_string());
    }
  }

  let client = Client::new();
  let response = client
    .post(endpoint)
    .header("Authorization", format!("Bearer {api_key}"))
    .multipart(form)
    .send()
    .map_err(|error| error.to_string())?;

  if !response.status().is_success() {
    let status = response.status();
    let text = response.text().unwrap_or_default();
    return Err(format!("Transcription API error: {status} {text}"));
  }

  let value: Value = response.json().map_err(|error| error.to_string())?;
  Ok(
    value
      .get("text")
      .and_then(|value| value.as_str())
      .unwrap_or("")
      .to_string(),
  )
}

fn emit_trpc_event(app: &tauri::AppHandle, path: &str, payload: Value) {
  let event = format!("trpc:{path}");
  let _ = app.emit_all(event, payload);
}

fn upsert_config(config: &mut Map<String, Value>, key: &str, input: &Value) {
  config.insert(key.to_string(), input.clone());
}

fn to_string_vec(values: &Vec<Value>) -> Vec<String> {
  values
    .iter()
    .filter_map(|value| value.as_str().map(|entry| entry.to_string()))
    .collect()
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FormatterConfigState {
  enabled: bool,
  model_id: Option<String>,
  fallback_model_id: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModeDictationState {
  auto_detect_enabled: bool,
  selected_language: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModeConfigState {
  id: String,
  name: String,
  is_default: bool,
  dictation: ModeDictationState,
  formatter_config: FormatterConfigState,
  custom_instructions: Option<String>,
  speech_model_id: Option<String>,
  app_bindings: Option<Vec<String>>,
  created_at: String,
  updated_at: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModesState {
  items: Vec<ModeConfigState>,
  active_mode_id: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadedSpeechModel {
  downloaded_at: i64,
  size_bytes: Option<i64>,
  checksum: Option<String>,
  local_path: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnboardingPreferencesState {
  feature_interests: Option<Vec<String>>,
  discovery_source: Option<String>,
  discovery_details: Option<String>,
  selected_model_type: Option<String>,
  model_recommendation: Option<OnboardingModelRecommendationState>,
  last_visited_screen: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnboardingModelRecommendationState {
  suggested: String,
  reason: String,
  followed: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnboardingState {
  completed_version: i64,
  completed_at: String,
  last_visited_screen: Option<String>,
  skipped_screens: Option<Vec<String>>,
  feature_interests: Option<Vec<String>>,
  discovery_source: Option<String>,
  selected_model_type: String,
  model_recommendation: Option<OnboardingModelRecommendationState>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthStatusState {
  is_authenticated: bool,
  user_email: Option<String>,
  user_name: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct SettingsState {
  preferences: PreferencesState,
  shortcuts: ShortcutsState,
  dictation: DictationState,
  telemetry: TelemetryState,
  recording: RecordingState,
  transcription: TranscriptionState,
  ui_theme: String,
  formatter_config: FormatterConfigState,
  modes: Option<ModesState>,
  models: ModelsState,
  downloaded_speech_models: HashMap<String, DownloadedSpeechModel>,
  synced_provider_models: Vec<Value>,
  model_providers_config: Map<String, Value>,
  transcription_providers_config: Map<String, Value>,
  onboarding_state: Option<OnboardingState>,
  onboarding_preferences: Option<OnboardingPreferencesState>,
  auth_status: Option<AuthStatusState>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct PreferencesState {
  launch_at_login: bool,
  minimize_to_tray: bool,
  show_widget_while_inactive: bool,
  show_in_dock: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct ShortcutsState {
  push_to_talk: Vec<String>,
  toggle_recording: Vec<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct DictationState {
  auto_detect_enabled: bool,
  selected_language: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct TelemetryState {
  enabled: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct RecordingState {
  preferred_microphone_name: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct TranscriptionState {
  preload_whisper_model: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct ModelsState {
  default_speech_model: String,
  default_language_model: String,
  default_embedding_model: String,
  selected_model: String,
}

struct RecordingSession {
  state: String,
  mode: String,
  audio_samples: Vec<f32>,
  started_at: Option<i64>,
}

impl RecordingSession {
  fn new() -> Self {
    Self {
      state: "idle".to_string(),
      mode: "idle".to_string(),
      audio_samples: Vec::new(),
      started_at: None,
    }
  }
}

struct AppState {
  settings: Mutex<SettingsState>,
  settings_path: PathBuf,
  machine_id: String,
  db: Mutex<Connection>,
  app_data_dir: PathBuf,
  #[allow(dead_code)]
  // Keep tray icon alive for the app lifetime.
  tray_icon: Mutex<Option<tauri::tray::TrayIcon>>,
  recording: Mutex<RecordingSession>,
}

impl SettingsState {
  fn with_defaults() -> Self {
    let mut settings = SettingsState::default();
    settings.preferences.launch_at_login = true;
    settings.preferences.show_widget_while_inactive = true;
    settings.preferences.show_in_dock = true;
    settings.dictation.auto_detect_enabled = true;
    settings.dictation.selected_language = "en".to_string();
    settings.transcription.preload_whisper_model = true;
    settings.ui_theme = "system".to_string();
    settings.telemetry.enabled = true;
    settings.formatter_config.enabled = false;
    if current_platform() == "darwin" {
      settings.shortcuts.push_to_talk = vec!["Fn".to_string()];
      settings.shortcuts.toggle_recording = vec!["Fn".to_string(), "Space".to_string()];
    } else {
      settings.shortcuts.push_to_talk = vec!["Ctrl".to_string(), "Win".to_string()];
      settings.shortcuts.toggle_recording = vec![
        "Ctrl".to_string(),
        "Win".to_string(),
        "Space".to_string(),
      ];
    }
    let fallback = build_fallback_mode(&settings);
    settings.modes = Some(ModesState {
      items: vec![fallback],
      active_mode_id: "default".to_string(),
    });
    settings
  }
}

fn now_iso() -> String {
  Utc::now().to_rfc3339()
}

fn env_flag(key: &str) -> bool {
  env::var(key)
    .map(|value| value == "true")
    .unwrap_or(false)
}

fn current_platform() -> &'static str {
  match env::consts::OS {
    "macos" => "darwin",
    "windows" => "win32",
    "linux" => "linux",
    other => other,
  }
}

fn build_fallback_mode(settings: &SettingsState) -> ModeConfigState {
  let now = now_iso();
  let selected_language = if settings.dictation.selected_language.is_empty() {
    "en".to_string()
  } else {
    settings.dictation.selected_language.clone()
  };
  ModeConfigState {
    id: "default".to_string(),
    name: "Default".to_string(),
    is_default: true,
    dictation: ModeDictationState {
      auto_detect_enabled: settings.dictation.auto_detect_enabled,
      selected_language,
    },
    formatter_config: settings.formatter_config.clone(),
    custom_instructions: None,
    speech_model_id: None,
    app_bindings: None,
    created_at: now.clone(),
    updated_at: now,
  }
}

fn get_modes_state(settings: &SettingsState) -> ModesState {
  if let Some(modes) = &settings.modes {
    if !modes.items.is_empty() {
      return modes.clone();
    }
  }
  let fallback = build_fallback_mode(settings);
  ModesState {
    items: vec![fallback],
    active_mode_id: "default".to_string(),
  }
}

fn parse_mode_dictation(value: Option<&Value>) -> Option<ModeDictationState> {
  let value = value?;
  Some(ModeDictationState {
    auto_detect_enabled: value.get("autoDetectEnabled")?.as_bool()?,
    selected_language: value
      .get("selectedLanguage")?
      .as_str()?
      .to_string(),
  })
}

fn parse_formatter_config(value: Option<&Value>) -> Option<FormatterConfigState> {
  let value = value?;
  let enabled = value.get("enabled")?.as_bool()?;
  let model_id = match value.get("modelId") {
    Some(Value::String(value)) => Some(value.to_string()),
    Some(Value::Null) => None,
    _ => None,
  };
  let fallback_model_id = match value.get("fallbackModelId") {
    Some(Value::String(value)) => Some(value.to_string()),
    Some(Value::Null) => None,
    _ => None,
  };
  Some(FormatterConfigState {
    enabled,
    model_id,
    fallback_model_id,
  })
}

fn load_available_models() -> Result<Vec<Value>, String> {
  let raw = include_str!("../assets/available-models.json");
  serde_json::from_str::<Vec<Value>>(raw).map_err(|error| error.to_string())
}

fn find_available_model<'a>(models: &'a [Value], model_id: &str) -> Option<&'a Value> {
  models.iter().find(|model| {
    model
      .get("id")
      .and_then(|value| value.as_str())
      .map(|value| value == model_id)
      .unwrap_or(false)
  })
}

fn open_external_url(url: &str) -> Result<(), String> {
  if url.is_empty() {
    return Ok(());
  }
  let result = match env::consts::OS {
    "macos" => Command::new("open").arg(url).status(),
    "windows" => Command::new("cmd").args(["/C", "start", "", url]).status(),
    _ => Command::new("xdg-open").arg(url).status(),
  };
  result
    .map_err(|error| error.to_string())
    .and_then(|status| {
      if status.success() {
        Ok(())
      } else {
        Err("Failed to open external URL".to_string())
      }
    })
}

fn provider_has_api_key(config: &Map<String, Value>, key: &str) -> bool {
  config
    .get(key)
    .and_then(|value| value.get("apiKey"))
    .and_then(|value| value.as_str())
    .map(|value| !value.trim().is_empty())
    .unwrap_or(false)
}

fn speech_model_value(model: &Value, downloaded: Option<&DownloadedSpeechModel>) -> Value {
  let id = model
    .get("id")
    .and_then(|value| value.as_str())
    .unwrap_or_default();
  let name = model
    .get("name")
    .and_then(|value| value.as_str())
    .unwrap_or(id);
  let provider = model
    .get("provider")
    .and_then(|value| value.as_str())
    .unwrap_or_default();
  let description = model
    .get("description")
    .and_then(|value| value.as_str())
    .unwrap_or_default();
  let setup = model
    .get("setup")
    .and_then(|value| value.as_str())
    .unwrap_or_default();
  let size = if setup == "offline" {
    model
      .get("sizeFormatted")
      .and_then(|value| value.as_str())
      .unwrap_or("")
      .to_string()
  } else if setup == "api" {
    "API".to_string()
  } else {
    "Cloud".to_string()
  };
  let speed = model.get("speed").and_then(|value| value.as_f64());
  let accuracy = model.get("accuracy").and_then(|value| value.as_f64());
  let timestamp = downloaded
    .map(|entry| entry.downloaded_at)
    .unwrap_or_else(now_unix_seconds);

  json!({
    "id": id,
    "name": name,
    "provider": provider,
    "type": "speech",
    "size": size,
    "context": Value::Null,
    "description": description,
    "localPath": downloaded.and_then(|entry| entry.local_path.clone()),
    "sizeBytes": downloaded.and_then(|entry| entry.size_bytes),
    "checksum": downloaded.and_then(|entry| entry.checksum.clone()),
    "downloadedAt": downloaded.map(|entry| to_millis(entry.downloaded_at)),
    "originalModel": Value::Null,
    "speed": speed,
    "accuracy": accuracy,
    "createdAt": to_millis(timestamp),
    "updatedAt": to_millis(timestamp),
    "setup": setup
  })
}

fn clear_missing_provider_defaults(settings: &mut SettingsState) {
  let ids: HashSet<String> = settings
    .synced_provider_models
    .iter()
    .filter_map(|model| {
      model
        .get("id")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
    })
    .collect();
  if !settings.models.default_language_model.is_empty()
    && !ids.contains(&settings.models.default_language_model)
  {
    settings.models.default_language_model.clear();
  }
  if !settings.models.default_embedding_model.is_empty()
    && !ids.contains(&settings.models.default_embedding_model)
  {
    settings.models.default_embedding_model.clear();
  }
}

#[derive(Clone)]
struct ScannedApp {
  name: String,
  bundle_id: String,
  icon_path: Option<PathBuf>,
}

fn list_installed_apps() -> Vec<Value> {
  if !cfg!(target_os = "macos") {
    return Vec::new();
  }
  let mut apps = Vec::new();
  let entries = match fs::read_dir("/Applications") {
    Ok(entries) => entries,
    Err(_) => return Vec::new(),
  };

  for entry in entries.flatten() {
    let path = entry.path();
    if path.extension().and_then(|value| value.to_str()) != Some("app") {
      continue;
    }
    if let Some(app) = read_plist(&path) {
      apps.push(app);
    }
  }

  apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

  apps
    .into_iter()
    .map(|app| {
      let icon = icon_data_url(app.icon_path.as_deref()).unwrap_or_default();
      json!({
        "name": app.name,
        "bundleId": app.bundle_id,
        "icon": icon
      })
    })
    .collect()
}

fn read_plist(app_path: &Path) -> Option<ScannedApp> {
  let plist_path = app_path.join("Contents").join("Info.plist");
  let output = Command::new("plutil")
    .args([
      "-convert",
      "json",
      "-o",
      "-",
      plist_path.to_str()?,
    ])
    .output()
    .ok()?;
  if !output.status.success() {
    return None;
  }
  let plist: Value = serde_json::from_slice(&output.stdout).ok()?;
  let bundle_id = plist
    .get("CFBundleIdentifier")
    .and_then(|value| value.as_str())?;
  let name = plist
    .get("CFBundleDisplayName")
    .and_then(|value| value.as_str())
    .or_else(|| plist.get("CFBundleName").and_then(|value| value.as_str()))
    .map(|value| value.to_string())
    .or_else(|| {
      app_path
        .file_stem()
        .and_then(|value| value.to_str())
        .map(|value| value.to_string())
    })?;
  let icon_file = plist
    .get("CFBundleIconFile")
    .and_then(|value| value.as_str())
    .or_else(|| plist.get("CFBundleIconName").and_then(|value| value.as_str()));
  let icon_path = icon_file.map(|value| {
    let icon_name = if value.ends_with(".icns") {
      value.to_string()
    } else {
      format!("{value}.icns")
    };
    app_path
      .join("Contents")
      .join("Resources")
      .join(icon_name)
  });
  Some(ScannedApp {
    name,
    bundle_id: bundle_id.to_string(),
    icon_path,
  })
}

fn icon_data_url(icon_path: Option<&Path>) -> Option<String> {
  let icon_path = icon_path?;
  if !icon_path.exists() {
    return None;
  }
  let tmp_path = env::temp_dir().join(format!("amical-icon-{}.png", Uuid::new_v4()));
  let status = Command::new("sips")
    .args([
      "-s",
      "format",
      "png",
      "-z",
      "64",
      "64",
      icon_path.to_str()?,
      "--out",
      tmp_path.to_str()?,
    ])
    .status()
    .ok()?;
  if !status.success() {
    let _ = fs::remove_file(&tmp_path);
    return None;
  }
  let data = fs::read(&tmp_path).ok()?;
  let _ = fs::remove_file(&tmp_path);
  Some(format!(
    "data:image/png;base64,{}",
    BASE64_ENGINE.encode(data)
  ))
}

#[derive(Serialize)]
struct SystemSpecs {
  cpu_model: Option<String>,
  cpu_cores: i64,
  cpu_threads: i64,
  cpu_speed_ghz: f64,
  memory_total_gb: f64,
  gpu_model: Option<String>,
  gpu_vendor: Option<String>,
}

fn read_sysctl_string(name: &str) -> Option<String> {
  let output = Command::new("sysctl")
    .args(["-n", name])
    .output()
    .ok()?;
  if !output.status.success() {
    return None;
  }
  let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
  if value.is_empty() {
    None
  } else {
    Some(value)
  }
}

fn read_sysctl_i64(name: &str) -> Option<i64> {
  read_sysctl_string(name).and_then(|value| value.parse::<i64>().ok())
}

fn get_system_specs() -> Option<SystemSpecs> {
  if !cfg!(target_os = "macos") {
    return None;
  }
  let cpu_model = read_sysctl_string("machdep.cpu.brand_string");
  let cpu_cores = read_sysctl_i64("hw.physicalcpu").unwrap_or(0);
  let cpu_threads = read_sysctl_i64("hw.logicalcpu").unwrap_or(0);
  let cpu_speed_ghz = read_sysctl_i64("hw.cpufrequency")
    .map(|value| value as f64 / 1_000_000_000.0)
    .unwrap_or(0.0);
  let memory_total_gb = read_sysctl_i64("hw.memsize")
    .map(|value| value as f64 / 1_073_741_824.0)
    .unwrap_or(0.0);

  Some(SystemSpecs {
    cpu_model,
    cpu_cores,
    cpu_threads,
    cpu_speed_ghz,
    memory_total_gb,
    gpu_model: None,
    gpu_vendor: None,
  })
}

fn recommended_local_model(cpu_model: &str) -> &'static str {
  let upper = cpu_model.to_uppercase();
  if upper.contains("M3 PRO")
    || upper.contains("M3 MAX")
    || upper.contains("M4")
    || upper.contains("M5")
    || upper.contains("M6")
  {
    return "whisper-large-v3-turbo";
  }
  if upper.contains("M2") || upper.contains("M3") {
    return "whisper-medium";
  }
  if upper.contains("M1") {
    return "whisper-small";
  }
  "whisper-base"
}

fn system_recommendation() -> Value {
  let specs = match get_system_specs() {
    Some(specs) => specs,
    None => {
      return json!({
        "suggested": "cloud",
        "reason": "Unable to detect system specifications. Cloud processing is recommended."
      });
    }
  };

  let cpu_model = specs.cpu_model.clone().unwrap_or_default();
  let upper = cpu_model.to_uppercase();
  let has_local_capacity = upper.contains("M2")
    || upper.contains("M3")
    || upper.contains("M4")
    || upper.contains("M5")
    || upper.contains("M6")
    || specs.memory_total_gb >= 16.0;

  if has_local_capacity {
    json!({
      "suggested": "local",
      "reason": "Your system has sufficient resources for local models, offering better privacy and offline capability.",
      "systemSpecs": specs
    })
  } else {
    json!({
      "suggested": "cloud",
      "reason": "Your system may experience slow performance with local models. Cloud processing is recommended for optimal speed.",
      "systemSpecs": specs
    })
  }
}

fn duration_until_next_2am() -> Duration {
  let now = Local::now();
  let (year, month, day) = (now.year(), now.month(), now.day());
  let today_two = Local
    .with_ymd_and_hms(year, month, day, 2, 0, 0)
    .single()
    .unwrap_or(now);
  let next = if now < today_two {
    today_two
  } else {
    let tomorrow = now + chrono::Duration::days(1);
    Local
      .with_ymd_and_hms(tomorrow.year(), tomorrow.month(), tomorrow.day(), 2, 0, 0)
      .single()
      .unwrap_or(tomorrow)
  };
  next
    .signed_duration_since(now)
    .to_std()
    .unwrap_or_else(|_| Duration::from_secs(0))
}

fn spawn_compaction_task(app_handle: tauri::AppHandle) {
  std::thread::spawn(move || {
    if cfg!(debug_assertions) {
      loop {
        std::thread::sleep(Duration::from_secs(300));
        {
          let state = app_handle.state::<AppState>();
          if let Ok(mut conn) = state.db.lock() {
            let _ = compact_all_notes(&mut conn);
          };
        }
      }
    } else {
      loop {
        let sleep_for = duration_until_next_2am();
        std::thread::sleep(sleep_for);
        {
          let state = app_handle.state::<AppState>();
          if let Ok(mut conn) = state.db.lock() {
            let _ = compact_all_notes(&mut conn);
          };
        }
      }
    }
  });
}

fn app_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
  let app_dir = app
    .path()
    .app_data_dir()
    .map_err(|error| error.to_string())?;
  fs::create_dir_all(&app_dir).map_err(|error| error.to_string())?;
  Ok(app_dir)
}

fn log_file_path(app_data_dir: &PathBuf) -> PathBuf {
  let filename = if cfg!(debug_assertions) {
    "vox-dev.log"
  } else {
    "vox.log"
  };
  app_data_dir.join("logs").join(filename)
}

fn reset_app_state(
  app: &tauri::AppHandle,
  app_data_dir: &PathBuf,
  db: &Mutex<Connection>,
) -> Result<(), String> {
  if let Ok(mut conn) = db.lock() {
    let mem_conn = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let old_conn = std::mem::replace(&mut *conn, mem_conn);
    drop(old_conn);
  }

  let db_path = app_data_dir.join("vox.db");
  let _ = fs::remove_file(&db_path);
  let _ = fs::remove_file(PathBuf::from(format!("{}-wal", db_path.to_string_lossy())));
  let _ = fs::remove_file(PathBuf::from(format!("{}-shm", db_path.to_string_lossy())));
  let _ = fs::remove_dir_all(app_data_dir.join("models"));
  let _ = fs::remove_dir_all(app_data_dir.join("recordings"));
  let _ = fs::remove_file(app_data_dir.join("settings.json"));
  let _ = fs::remove_dir_all(app_data_dir.join("logs"));

  app.exit(0);
  Ok(())
}

fn create_widget_window(app: &tauri::AppHandle, visible: bool) -> Result<(), String> {
  if app.get_webview_window("widget").is_some() {
    return Ok(());
  }

  let monitor = app
    .primary_monitor()
    .map_err(|error| error.to_string())?
    .or_else(|| app.current_monitor().ok().flatten())
    .ok_or("No monitor available")?;
  let monitor_size = monitor.size();
  let monitor_position = monitor.position();
  let widget_width = 160.0;
  let widget_height = 64.0;
  let margin = 16.0;
  let x = monitor_position.x as f64 + (monitor_size.width as f64 - widget_width) / 2.0;
  let y = monitor_position.y as f64 + monitor_size.height as f64 - widget_height - margin;

  let window = WebviewWindowBuilder::new(app, "widget", WebviewUrl::App("widget.html".into()))
    .title("Vox Widget")
    .decorations(false)
    .transparent(true)
    .resizable(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .visible(visible)
    .inner_size(widget_width, widget_height)
    .position(x, y)
    .build()
    .map_err(|error| error.to_string())?;

  if cfg!(target_os = "macos") {
    let _ = window.set_visible_on_all_workspaces(true);
  }

  Ok(())
}

fn init_database(path: &PathBuf) -> Result<Connection, String> {
  let conn = Connection::open(path).map_err(|error| error.to_string())?;
  conn
    .execute_batch(
      "PRAGMA foreign_keys = ON;
       CREATE TABLE IF NOT EXISTS notes (
         id INTEGER PRIMARY KEY AUTOINCREMENT,
         title TEXT NOT NULL,
         content TEXT DEFAULT '',
         icon TEXT,
         created_at INTEGER NOT NULL DEFAULT (unixepoch()),
         updated_at INTEGER NOT NULL DEFAULT (unixepoch())
       );
       CREATE TABLE IF NOT EXISTS yjs_updates (
         id INTEGER PRIMARY KEY AUTOINCREMENT,
         note_id INTEGER NOT NULL,
         update_data BLOB NOT NULL,
         created_at INTEGER NOT NULL DEFAULT (unixepoch()),
         FOREIGN KEY(note_id) REFERENCES notes(id) ON DELETE CASCADE
       );
       CREATE INDEX IF NOT EXISTS yjs_updates_note_id_idx ON yjs_updates(note_id);
       CREATE TABLE IF NOT EXISTS transcriptions (
         id INTEGER PRIMARY KEY AUTOINCREMENT,
         text TEXT NOT NULL,
         timestamp INTEGER NOT NULL DEFAULT (unixepoch()),
         language TEXT DEFAULT 'en',
         audio_file TEXT,
         confidence REAL,
         duration INTEGER,
         speech_model TEXT,
         formatting_model TEXT,
         meta TEXT,
         created_at INTEGER NOT NULL DEFAULT (unixepoch()),
         updated_at INTEGER NOT NULL DEFAULT (unixepoch())
       );
       CREATE TABLE IF NOT EXISTS vocabulary (
         id INTEGER PRIMARY KEY AUTOINCREMENT,
         word TEXT NOT NULL UNIQUE,
         replacement_word TEXT,
         is_replacement INTEGER DEFAULT 0,
         date_added INTEGER NOT NULL DEFAULT (unixepoch()),
         usage_count INTEGER DEFAULT 0,
         created_at INTEGER NOT NULL DEFAULT (unixepoch()),
         updated_at INTEGER NOT NULL DEFAULT (unixepoch())
       );",
    )
    .map_err(|error| error.to_string())?;
  Ok(conn)
}

fn load_settings(path: &PathBuf) -> SettingsState {
  if !path.exists() {
    return SettingsState::with_defaults();
  }
  match fs::read_to_string(path) {
    Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|_| {
      eprintln!("Failed to parse settings file. Using defaults.");
      SettingsState::with_defaults()
    }),
    Err(_) => SettingsState::with_defaults(),
  }
}

fn persist_settings(path: &PathBuf, settings: &SettingsState) -> Result<(), String> {
  let contents =
    serde_json::to_string_pretty(settings).map_err(|error| error.to_string())?;
  let tmp_path = path.with_extension("tmp");
  fs::write(&tmp_path, contents).map_err(|error| error.to_string())?;
  fs::rename(&tmp_path, path).map_err(|error| error.to_string())?;
  Ok(())
}

fn main() {
  tauri::Builder::default()
    .plugin(tauri_plugin_opener::init())
    .plugin(tauri_plugin_dialog::init())
    .invoke_handler(tauri::generate_handler![
      trpc,
      audio_data_chunk,
      notes_save_yjs_update,
      notes_load_yjs_updates,
      notes_replace_yjs_updates
    ])
    .setup(|app| {
      let app_dir = app_data_dir(app.handle())?;
      let settings_path = app_dir.join("settings.json");
      let settings = load_settings(&settings_path);
      let _ = persist_settings(&settings_path, &settings);
      let machine_id_path = app_dir.join("machine-id");
      let machine_id = fs::read_to_string(&machine_id_path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
          let id = Uuid::new_v4().to_string();
          let _ = fs::write(&machine_id_path, &id);
          id
        });
      let db_path = app_dir.join("vox.db");
      let db = init_database(&db_path)?;
      let version_label = MenuItem::with_id(
        app,
        "version",
        format!("Version {}", app.package_info().version),
        false,
        None::<&str>,
      )
      .map_err(|error| error.to_string())?;
      let mut tray_menu_builder = MenuBuilder::new(app)
        .text("open-console", "Open Console")
        .separator();
      tray_menu_builder = if cfg!(target_os = "macos") {
        tray_menu_builder.about(None)
      } else {
        tray_menu_builder.text("about", "About")
      };
      let tray_menu = tray_menu_builder
        .item(&version_label)
        .separator()
        .text("quit", "Quit")
        .build()
        .map_err(|error| error.to_string())?;
      let mut tray_builder = TrayIconBuilder::new()
        .menu(&tray_menu)
        .tooltip("Vox")
        .show_menu_on_left_click(true)
        .on_menu_event(|app: &AppHandle, event: MenuEvent| match event.id().as_ref() {
          "open-console" => {
            if let Some(window) = app.get_webview_window("main") {
              let _ = window.show();
              let _ = window.set_focus();
            }
          }
          "about" => {
            // Fallback for non-macOS platforms without a predefined About menu item.
            let _ = app.get_webview_window("main").map(|window| window.show());
          }
          "quit" => {
            app.exit(0);
          }
          _ => {}
        })
        .on_tray_icon_event(|tray: &tauri::tray::TrayIcon, event| {
          if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
          } = event
          {
            let app = tray.app_handle();
            if let Some(window) = app.get_webview_window("main") {
              let _ = window.show();
              let _ = window.set_focus();
            }
          }
        });
      let tray_icon_image = if cfg!(target_os = "macos") {
        Image::from_bytes(include_bytes!("../../assets/iconTemplate.png")).ok()
      } else if cfg!(target_os = "windows") {
        Image::from_bytes(include_bytes!("../../assets/icon-256x256.png")).ok()
      } else {
        Image::from_bytes(include_bytes!("../../assets/icon-512x512.png")).ok()
      };
      if let Some(icon) = tray_icon_image {
        tray_builder = tray_builder.icon(icon).icon_as_template(cfg!(target_os = "macos"));
      }
      let tray_icon = tray_builder
        .build(app)
        .map_err(|error| error.to_string())?;
      let show_widget = settings.preferences.show_widget_while_inactive;
      app.manage(AppState {
        settings: Mutex::new(settings),
        settings_path,
        machine_id,
        db: Mutex::new(db),
        app_data_dir: app_dir,
        tray_icon: Mutex::new(Some(tray_icon)),
        recording: Mutex::new(RecordingSession::new()),
      });
      spawn_compaction_task(app.handle().clone());
      if let Err(error) = create_widget_window(app.handle(), show_widget) {
        eprintln!("Failed to create widget window: {error}");
      }
      Ok(())
    })
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
