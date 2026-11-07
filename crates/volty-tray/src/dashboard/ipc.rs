use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::DashboardSettingsUpdate;

use super::management::{CleanupKind, DashboardBackend};

#[derive(Debug, Deserialize)]
struct IpcRequest {
    #[serde(flatten)]
    command: IpcCommand,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "command", content = "payload", rename_all = "snake_case")]
enum IpcCommand {
    GetDashboardData {
        days: u32,
    },
    PreviewCleanup {
        kind: CleanupKind,
        #[serde(default)]
        days: Option<u32>,
        #[serde(default)]
        identity: Option<String>,
    },
    ExecuteCleanup {
        kind: CleanupKind,
        #[serde(default)]
        days: Option<u32>,
        #[serde(default)]
        identity: Option<String>,
    },
    ForgetDevice {
        identity: String,
        include_preferences: bool,
    },
    SetHistoryRecording {
        identity: String,
        enabled: bool,
    },
    UpdateSettings {
        settings: DashboardSettingsUpdate,
    },
    OpenSettings {},
    TestNotification {},
    ListArtifacts {},
    DeleteArtifact {
        id: String,
    },
}

#[derive(Serialize)]
struct IpcResponse {
    id: Value,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub(super) fn handle_message(message: &str, backend: &DashboardBackend) -> String {
    let parsed = match serde_json::from_str::<Value>(message) {
        Ok(value) => value,
        Err(error) => return error_response(Value::Null, format!("invalid IPC JSON: {error}")),
    };
    let id = parsed.get("id").cloned().unwrap_or(Value::Null);
    if !matches!(id, Value::String(_) | Value::Number(_)) {
        return error_response(id, "IPC request id must be a string or number".to_owned());
    }
    let request = match serde_json::from_value::<IpcRequest>(parsed) {
        Ok(request) => request,
        Err(error) => return error_response(id, format!("invalid dashboard IPC request: {error}")),
    };
    match dispatch(request, backend) {
        Ok(result) => serialize_response(IpcResponse {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }),
        Err(error) => error_response(id, error),
    }
}

fn dispatch(request: IpcRequest, backend: &DashboardBackend) -> Result<Value, String> {
    match request.command {
        IpcCommand::GetDashboardData { days } => {
            serde_json::to_value(backend.get_dashboard_data(days).map_err(display_error)?)
                .map_err(display_error)
        }
        IpcCommand::PreviewCleanup {
            kind,
            days,
            identity,
        } => serde_json::to_value(
            backend
                .preview_cleanup(kind, days, identity.as_deref())
                .map_err(display_error)?,
        )
        .map_err(display_error),
        IpcCommand::ExecuteCleanup {
            kind,
            days,
            identity,
        } => serde_json::to_value(
            backend
                .execute_cleanup(kind, days, identity.as_deref())
                .map_err(display_error)?,
        )
        .map_err(display_error),
        IpcCommand::ForgetDevice {
            identity,
            include_preferences,
        } => serde_json::to_value(
            backend
                .forget_device(&identity, include_preferences)
                .map_err(display_error)?,
        )
        .map_err(display_error),
        IpcCommand::SetHistoryRecording { identity, enabled } => serde_json::to_value(
            backend
                .set_history_recording(&identity, enabled)
                .map_err(display_error)?,
        )
        .map_err(display_error),
        IpcCommand::UpdateSettings { settings } => {
            serde_json::to_value(backend.update_settings(&settings).map_err(display_error)?)
                .map_err(display_error)
        }
        IpcCommand::OpenSettings {} => {
            backend.open_settings().map_err(display_error)?;
            Ok(Value::Null)
        }
        IpcCommand::TestNotification {} => {
            backend.test_notification().map_err(display_error)?;
            Ok(Value::Null)
        }
        IpcCommand::ListArtifacts {} => {
            serde_json::to_value(backend.list_artifacts().map_err(display_error)?)
                .map_err(display_error)
        }
        IpcCommand::DeleteArtifact { id } => {
            serde_json::to_value(backend.delete_artifact(&id).map_err(display_error)?)
                .map_err(display_error)
        }
    }
}

fn display_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn error_response(id: Value, error: String) -> String {
    serialize_response(IpcResponse {
        id,
        ok: false,
        result: None,
        error: Some(error),
    })
}

fn serialize_response(response: IpcResponse) -> String {
    serde_json::to_string(&response).unwrap_or_else(|_| {
        r#"{"id":null,"ok":false,"error":"could not serialize dashboard response"}"#.to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::management::DashboardBackend;
    use std::path::PathBuf;

    fn backend() -> DashboardBackend {
        DashboardBackend::new(PathBuf::from("nonexistent-history.sqlite3"), false)
    }

    #[test]
    fn typed_ipc_accepts_the_shared_command_shapes_and_correlates_responses() {
        let response = handle_message(
            r#"{"id":17,"command":"preview_cleanup","payload":{"kind":"inactive_devices","days":14,"identity":"hid:keyboard#1"}}"#,
            &backend(),
        );
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["id"], 17);
        assert_eq!(response["ok"], true);
        assert_eq!(response["result"]["kind"], "inactive_devices");
        assert_eq!(response["result"]["devices"], 0);

        let response = handle_message(
            r#"{"id":"history-preview","command":"preview_cleanup","payload":{"kind":"device_history","identity":"hid:keyboard#1"}}"#,
            &backend(),
        );
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["id"], "history-preview");
        assert_eq!(response["ok"], true);
        assert_eq!(response["result"]["samples"], 0);
        assert_eq!(response["result"]["devices"], 0);
        assert_eq!(response["result"]["first_at"], Value::Null);
        assert_eq!(response["result"]["last_at"], Value::Null);
        assert_eq!(
            response["result"]["date_range"],
            "No samples recorded for hid:keyboard#1"
        );

        let response = handle_message(
            r#"{"id":"refresh-1","command":"get_dashboard_data","payload":{"days":0}}"#,
            &backend(),
        );
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["id"], "refresh-1");
        assert_eq!(response["ok"], false);
        assert!(response["error"].as_str().unwrap().contains("days must be"));
    }

    #[test]
    fn typed_ipc_rejects_unknown_commands_and_reports_errors() {
        let response = handle_message(
            r#"{"id":"bad","command":"run_sql","payload":{"query":"DELETE FROM samples"}}"#,
            &backend(),
        );
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["id"], "bad");
        assert_eq!(response["ok"], false);
        assert!(response["error"]
            .as_str()
            .unwrap()
            .contains("invalid dashboard IPC request"));
    }

    #[test]
    fn settings_ipc_requires_the_complete_supported_settings_shape() {
        let valid = serde_json::from_str::<IpcRequest>(
            r#"{"id":"settings","command":"update_settings","payload":{"settings":{"low_battery_thresholds":[20,10,5],"low_battery_cooldown_minutes":120,"notifications_enabled":true,"tray_icon_style":"battery","tray_icon_theme":"system","show_percentage_in_battery_icon":false,"pinned_device":null,"start_on_boot":true}}}"#,
        )
        .unwrap();
        assert!(matches!(valid.command, IpcCommand::UpdateSettings { .. }));

        let invalid = serde_json::from_str::<IpcRequest>(
            r#"{"id":"settings","command":"update_settings","payload":{"settings":{"low_battery_thresholds":[20],"low_battery_cooldown_minutes":120,"notifications_enabled":true,"tray_icon_style":"battery","tray_icon_theme":"system","show_percentage_in_battery_icon":false,"pinned_device":null,"start_on_boot":true,"unexpected":true}}}"#,
        );
        assert!(invalid.is_err());
    }

    #[test]
    fn typed_payload_deserializes_exact_identity_and_preference_fields() {
        let request: IpcRequest = serde_json::from_str(
            r#"{"id":"x","command":"set_history_recording","payload":{"identity":"hid:a#1","enabled":false}}"#,
        )
        .unwrap();
        assert_eq!(
            request.command,
            IpcCommand::SetHistoryRecording {
                identity: "hid:a#1".to_owned(),
                enabled: false,
            }
        );
    }
}
